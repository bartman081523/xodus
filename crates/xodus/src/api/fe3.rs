//! FE3 client for Microsoft Store package discovery (Windows Update delivery).
//!
//! The `storeedgefd` product endpoint yields a `FulfillmentData` with the
//! `WuCategoryId` of a Store product; this module resolves that category to
//! concrete, unencrypted download URLs through three SOAP round trips against
//! the (undocumented) FE3 endpoints:
//!
//! 1. `GetCookie` — session cookie, no authorization.
//! 2. `SyncUpdates` — enumerates the packages of a category, including file
//!    names, sizes and digests. FE3 describes each package twice: the update
//!    identity lives under `NewUpdates/UpdateInfo`, the file metadata in a
//!    parallel `ExtendedUpdateInfo/Updates/Update` tree; both are joined by
//!    the numeric FE3 update id. Each record's inner `Xml` node carries that
//!    half's content in one of two wire forms — as real child elements, or as
//!    a single XML-escaped text node. Both were captured on 2026-10-12, so
//!    the `Xml` content is collected as one unit and sub-parsed from a
//!    uniform inner-XML string (see `XmlCollector`).
//! 3. `GetExtendedUpdateInfo2` (`.../client.asmx/secured`) — resolves one
//!    update identity to tokenized delivery URLs on
//!    `tlu.dl.delivery.mp.microsoft.com`.
//!
//! Provenance: the call shapes and the XML request templates are adapted from
//! the MIT-licensed `alt-app-installer` project (© 2022 Jishnu M); the
//! templates in `resources/fe3/` are verbatim copies, with that license kept
//! alongside as `resources/fe3/LICENSE.alt-app-installer`. Response shapes are
//! modeled against live captures (2026-10-12,
//! `scratches/fe3_spike/transcripts/`).
//!
//! Keep the protocol details isolated here: the FE3 endpoints are
//! undocumented and may change without notice.

use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesEnd, BytesRef, BytesStart, BytesText, Event};
use thiserror::Error;

use crate::models::fe3::Fe3File;

pub const FE3_CLIENT_WEB_SERVICE: &str =
    "https://fe3cr.delivery.mp.microsoft.com/ClientWebService/client.asmx";
pub const FE3_CLIENT_WEB_SERVICE_SECURED: &str =
    "https://fe3cr.delivery.mp.microsoft.com/ClientWebService/client.asmx/secured";

const FE3_SOAP_CONTENT_TYPE: &str = "application/soap+xml; charset=utf-8";

const FE3_GET_COOKIE_TEMPLATE: &str = include_str!("../../resources/fe3/GetCookie.xml");
const FE3_SYNC_UPDATES_TEMPLATE: &str = include_str!("../../resources/fe3/WUIDRequest.xml");
const FE3_FILE_URL_TEMPLATE: &str = include_str!("../../resources/fe3/FE3FileUrl.xml");

/// The `secured` step answers each request with two URLs: the tokenized
/// delivery URL of the file itself, plus a fixed-shape, tokenless URL of
/// exactly 99 characters pointing to its blockmap. The 99-character form is
/// the only blockmap shape, so any other length resolves a real file
/// (verified live 13/13, 2026-10-12).
const SECURED_BLOCKMAP_URL_LEN: usize = 99;

/// Downloaded 2026-10-12 from the official Microsoft PKI repository
/// (https://www.microsoft.com/pkiops/certs/Microsoft%20ECC%20Product%20Root%20Certificate%20Authority%202018.crt,
/// SHA-1 `06:F1:AA:33:0B:92:7B:75:3A:40:E6:8C:DF:22:E3:4B:CB:EF:33:52` as published there,
/// SHA-256 `CA:CA:93:B9:D2:3D:2B:6F:A7:6E:8B:84:71:93:1E:0D:F3:EC:6F:63:AF:3C:DB:B9:36:C4:19:54:A1:87:23:26`,
/// re-computed locally), then proven against the live-served chain:
/// `openssl verify -CAfile <this> -untrusted <intermediate> <leaf>` → `OK`.
const FE3_ROOT_PEM: &str = include_str!("../../resources/fe3/MicrosoftECCProductRootCA2018.pem");

#[derive(Debug, Error)]
pub enum Fe3Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("xml error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("text escaping error: {0}")]
    Escape(#[from] quick_xml::escape::EscapeError),
    #[error("unexpected FE3 response: {0}")]
    Protocol(&'static str),
}

/// Adds the private Microsoft root the FE3 endpoints chain to
/// (`Microsoft ECC Product Root Certificate Authority 2018`) as an extra
/// trust anchor to the client used for this module's calls.
///
/// This widens trust instead of weakening validation: certificate
/// verification itself stays untouched and public CAs keep working. The FE3
/// chain is genuine - it just ends at that private root, which Windows
/// already ships but Linux trust stores do not (`UnknownIssuer`
/// otherwise). The file URLs the protocol returns are plain `http`, which
/// is why they only get digests, not TLS, for protection.
pub fn add_fe3_trust(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let root = reqwest::Certificate::from_pem(FE3_ROOT_PEM.as_bytes())
        .expect("embedded Microsoft ECC Product Root CA 2018 PEM must parse");
    builder.add_root_certificate(root)
}

/// Requests an FE3 session cookie for the SOAP requests of this module.
pub async fn get_cookie(client: &reqwest::Client) -> Result<String, Fe3Error> {
    let response = post_soap(client, FE3_CLIENT_WEB_SERVICE, FE3_GET_COOKIE_TEMPLATE).await?;
    parse_cookie(&response)
}

/// Enumerates all unencrypted files of a Store product's `WuCategoryId`.
///
/// `cookie` comes from [`get_cookie`], `wu_category_id` from the product
/// endpoint's `FulfillmentData`, and `release` is the Windows Update ring
/// (`"Retail"` for stable Store content). Returns one entry per resolvable
/// package file (main package as well as dependencies), in server document
/// order.
pub async fn sync_updates(
    client: &reqwest::Client,
    cookie: &str,
    wu_category_id: &str,
    release: &str,
) -> Result<Vec<Fe3File>, Fe3Error> {
    let request = fill_template(
        FE3_SYNC_UPDATES_TEMPLATE,
        &[("{0}", cookie), ("{1}", wu_category_id), ("{2}", release)],
    );
    let response = post_soap(client, FE3_CLIENT_WEB_SERVICE, &request).await?;
    parse_sync_updates(&response)
}

/// Resolves the tokenized delivery URL of one file's main payload.
///
/// Returns `None` when the response carries no non-blockmap URL. URLs are
/// time-limited (query parameter `P1`, roughly a month); fetch promptly or
/// call again to refresh.
pub async fn file_url(
    client: &reqwest::Client,
    file: &Fe3File,
    release: &str,
) -> Result<Option<String>, Fe3Error> {
    let urls = file_urls(client, file, release).await?;
    Ok(urls
        .into_iter()
        .find(|url| url.len() != SECURED_BLOCKMAP_URL_LEN))
}

/// Resolves every URL reported by the `secured` step for one file, including
/// the blockmap (see [`SECURED_BLOCKMAP_URL_LEN`]).
pub async fn file_urls(
    client: &reqwest::Client,
    file: &Fe3File,
    release: &str,
) -> Result<Vec<String>, Fe3Error> {
    let request = fill_template(
        FE3_FILE_URL_TEMPLATE,
        &[
            ("{0}", &file.update_id),
            ("{1}", &file.revision_number),
            ("{2}", release),
        ],
    );
    let response = post_soap(client, FE3_CLIENT_WEB_SERVICE_SECURED, &request).await?;
    parse_file_locations(&response)
}

/// POSTs a SOAP envelope to a FE3 endpoint, forcing the SOAP content type.
async fn post_soap(client: &reqwest::Client, url: &str, body: &str) -> Result<String, Fe3Error> {
    let response = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, FE3_SOAP_CONTENT_TYPE)
        .body(body.to_owned())
        .send()
        .await?
        .error_for_status()?;
    Ok(response.text().await?)
}

/// Substitutes the indexed `{0}`..`{2}` placeholders of a FE3 SOAP template.
fn fill_template(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut filled = template.to_owned();
    for (placeholder, value) in replacements {
        filled = filled.replace(placeholder, value);
    }
    filled
}

/// FE3 escapes most content itself but leaves raw, unescaped `&` inside URL
/// text nodes (e.g. the `?P1=...&P2=...` query of a `secured` response), which
/// is not well-formed XML. Escape stray ampersands before parsing.
///
/// Idempotent: well-formed entity references are left untouched.
fn escape_stray_ampersands(input: &str) -> String {
    if !input.contains('&') {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let mut escaped = String::with_capacity(input.len());
    for (index, ch) in input.char_indices() {
        if ch == '&' && !is_entity_reference(&bytes[index + 1..]) {
            escaped.push_str("&amp;");
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

/// Checks whether the bytes following an `&` start an entity reference: an
/// alphanumeric name or a numeric (`#...`) character reference, both
/// terminated by `;`.
fn is_entity_reference(rest: &[u8]) -> bool {
    let Some(&first) = rest.first() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'#') {
        return false;
    }
    // For a numeric character reference the `#` itself is no part of the
    // alphanumeric name scanned below; the scan starts after it.
    let name_start = usize::from(first == b'#');
    let end = rest
        .iter()
        .skip(name_start)
        .position(|b| !b.is_ascii_alphanumeric())
        .map_or(rest.len(), |position| position + name_start);
    rest.get(end) == Some(&b';')
}

/// One text node captured while walking a response, filled between a start
/// element and its matching end element.
#[derive(Default)]
struct TextCapture {
    active: bool,
    content: String,
}

impl TextCapture {
    /// Starts capturing at a `<tag>` that precedes the wanted text node.
    fn begin(&mut self) {
        self.active = true;
        self.content.clear();
    }

    fn feed(&mut self, text: &BytesText<'_>) -> Result<(), quick_xml::escape::EscapeError> {
        if self.active {
            self.content
                .push_str(unescape(text.as_ref().trim())?.as_ref());
        }
        Ok(())
    }

    /// Adds an entity reference (`&name;`) that the reader splits out of the
    /// surrounding text as its own [`Event::GeneralRef`] event.
    fn feed_reference(
        &mut self,
        reference: &BytesRef<'_>,
    ) -> Result<(), quick_xml::escape::EscapeError> {
        if self.active {
            self.content
                .push_str(&unescape(&format!("&{};", reference.as_ref()))?);
        }
        Ok(())
    }

    /// Stops capturing after any sibling or parent boundary and yields the
    /// text captured so far.
    fn finish(&mut self) -> Option<String> {
        if self.active {
            self.active = false;
            (!self.content.is_empty()).then(|| self.content.clone())
        } else {
            None
        }
    }

    fn stop(&mut self) {
        self.active = false;
    }
}

/// Local (prefix-stripped) name of a start or empty element.
fn start_name<'a>(element: &'a BytesStart<'_>) -> &'a str {
    element.local_name().into_inner()
}

/// Local (prefix-stripped) name of an end element.
fn end_name<'a>(element: &'a BytesEnd<'_>) -> &'a str {
    element.local_name().into_inner()
}

/// Value of the first attribute with the given local name, if present.
fn element_attr(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().find_map(|attr| {
        let Ok(attr) = attr else {
            return None;
        };
        if attr.key.local_name().into_inner() != name {
            return None;
        }
        attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .ok()
            .map(|value| value.into_owned())
    })
}

/// Feeds an attribute of a `File` element into its named field.
fn file_attr(attr: &Attribute<'_>, row: &mut PartialFileRow) -> Result<(), Fe3Error> {
    match attr.key.local_name().into_inner() {
        "FileName" => row.file_name = Some(attr_value(attr)?),
        "Digest" => row.digest = Some(attr_value(attr)?),
        "DigestAlgorithm" => row.digest_algorithm = Some(attr_value(attr)?),
        "Size" => row.size = attr_value(attr)?.parse().ok(),
        "Modified" => row.modified = Some(attr_value(attr)?),
        "InstallerSpecificIdentifier" => {
            row.installer_specific_identifier = Some(attr_value(attr)?)
        }
        _ => {}
    }
    Ok(())
}

fn attr_value(attr: &Attribute<'_>) -> Result<String, Fe3Error> {
    Ok(attr
        .normalized_value(quick_xml::XmlVersion::Implicit1_0)?
        .into_owned())
}

fn parse_cookie(response: &str) -> Result<String, Fe3Error> {
    let escaped = escape_stray_ampersands(response);
    let mut reader = Reader::from_str(&escaped);
    reader.config_mut().trim_text(true);

    let mut capture = TextCapture::default();
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                if start_name(&element) == "EncryptedData" {
                    capture.begin();
                }
            }
            Event::Text(text) => capture.feed(&text)?,
            Event::GeneralRef(reference) => capture.feed_reference(&reference)?,
            Event::CData(text) => {
                if capture.active {
                    capture.content.push_str(text.as_ref().trim());
                }
            }
            Event::End(element) => {
                if end_name(&element) == "EncryptedData" && capture.active {
                    return capture
                        .finish()
                        .ok_or(Fe3Error::Protocol("empty EncryptedData cookie"));
                }
                capture.stop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Err(Fe3Error::Protocol("no EncryptedData in GetCookie response"))
}

/// Inner XML of a `SyncUpdates` record's `Xml` node, collected as a unit
/// between the `Xml` start and its matching end.
///
/// The server sends that content in two wire forms: as real child elements
/// (collected here by re-serializing events verbatim) or as a single,
/// XML-escaped text node (collected raw and decoded only in [`finish`]) —
/// [`Self::finish`] turns both into the same real inner-XML string.
#[derive(Default)]
struct XmlCollector {
    active: bool,
    saw_element: bool,
    /// Depth of nested `<Xml>` starts inside the collected content;
    /// defensively tracked, though not observed in live captures.
    depth: usize,
    raw: String,
}

impl XmlCollector {
    /// Starts collecting at an `<Xml>` start inside a `SyncUpdates` record.
    fn begin(&mut self) {
        self.active = true;
        self.saw_element = false;
        self.depth = 0;
        self.raw.clear();
    }

    /// Adds an inner start or self-closing element, re-serialized verbatim
    /// (`Empty` event content carries the attributes but not its `/`).
    fn feed_start(&mut self, element: &BytesStart<'_>, empty: bool) {
        if !self.active {
            return;
        }
        self.saw_element = true;
        self.raw.push('<');
        self.raw.push_str(element.as_ref());
        self.raw.push_str(if empty { "/>" } else { ">" });
        if !empty && start_name(element) == "Xml" {
            self.depth += 1;
        }
    }

    /// Adds an inner closing tag; a nested `<Xml>` end returns to the
    /// collected depth instead of finishing (see [`Self::finish`]).
    fn close(&mut self, name: &str) {
        if !self.active {
            return;
        }
        self.saw_element = true;
        self.raw.push_str("</");
        self.raw.push_str(name);
        self.raw.push('>');
        if name == "Xml" && self.depth > 0 {
            self.depth -= 1;
        }
    }

    /// Adds an inner text node, kept raw (still escaped), so the two wire
    /// forms reassemble identically regardless of how the reader splits
    /// entities out of the text.
    fn feed_text(&mut self, text: &BytesText<'_>) {
        if self.active {
            self.raw.push_str(text.as_ref());
        }
    }

    /// Adds an entity reference the reader split out of the surrounding
    /// text, restored to its `&name;` form.
    fn feed_reference(&mut self, reference: &BytesRef<'_>) {
        if self.active {
            self.raw.push('&');
            self.raw.push_str(reference.as_ref());
            self.raw.push(';');
        }
    }

    /// Stops collecting and yields the inner XML: element content as-is,
    /// text-only content decoded from its escaped form. `None` for an empty
    /// or missing content — the record then simply carries no fields.
    fn finish(&mut self) -> Result<Option<String>, Fe3Error> {
        if !self.active {
            return Ok(None);
        }
        self.active = false;
        self.depth = 0;
        let raw = std::mem::take(&mut self.raw);
        if raw.is_empty() {
            return Ok(None);
        }
        if self.saw_element {
            return Ok(Some(raw));
        }
        Ok(Some(unescape(raw.trim())?.into_owned()))
    }
}

/// Update identity half of a `SyncUpdates` entry
/// (`NewUpdates/UpdateInfo`, joined by its `ID`).
#[derive(Default)]
struct PartialIdentity {
    id: Option<String>,
    update_id: Option<String>,
    revision_number: Option<String>,
    secured: bool,
}

/// File half of a `SyncUpdates` entry
/// (`ExtendedUpdateInfo/Updates/Update/Files`).
#[derive(Default)]
struct PartialFileRow {
    file_name: Option<String>,
    digest: Option<String>,
    digest_algorithm: Option<String>,
    size: Option<u64>,
    modified: Option<String>,
    installer_specific_identifier: Option<String>,
}

impl PartialFileRow {
    fn complete(&self) -> Option<(String, String, u64, String, String, String)> {
        Some((
            self.file_name.clone()?,
            self.installer_specific_identifier.clone()?,
            self.size?,
            self.digest.clone()?,
            self.digest_algorithm.clone()?,
            self.modified.clone()?,
        ))
    }
}

#[derive(Default)]
struct PartialUpdate {
    id: Option<String>,
    identity_name: Option<String>,
    files: Vec<PartialFileRow>,
}

/// Completes an update identity half of a `SyncUpdates` entry: only entries
/// resolved through the `secured` step (marked by `SecuredFragment`) with all
/// three identity parts join with a file half.
fn seal_identity(info: PartialIdentity) -> Option<(String, (String, String))> {
    if !info.secured {
        return None;
    }
    let PartialIdentity {
        id,
        update_id,
        revision_number,
        ..
    } = info;
    Some((id?, (update_id?, revision_number?)))
}

/// Completes a file half of a `SyncUpdates` entry. Updates without file rows
/// (e.g. pure applicability carriers) cannot be resolved.
fn seal_update(pending: PartialUpdate) -> Option<(String, PartialUpdate)> {
    if pending.files.is_empty() {
        return None;
    }
    let id = pending.id.clone()?;
    Some((id, pending))
}

/// Applies a `SyncUpdates` record-boundary start/empty element to the parse
/// state.
///
/// Runs for both `Event::Start` and `Event::Empty`; `UpdateInfo`/`Update`
/// container starts reset the pending halves. The `ID` text capture stays in
/// the caller because `Event::Empty` elements carry no text. Content-level
/// nodes (`UpdateIdentity`, `SecuredFragment`, `ExtendedProperties`, `File`)
/// live inside a record's `Xml` node and are sub-parsed from the collected
/// inner XML instead, so both wire forms (real elements and escaped text)
/// flow through the same code; see `XmlCollector`.
fn handle_element(
    element: &BytesStart<'_>,
    parent: Option<&str>,
    identity: &mut Option<PartialIdentity>,
    update: &mut Option<PartialUpdate>,
    identities: &mut Vec<(String, (String, String))>,
    updates: &mut Vec<(String, PartialUpdate)>,
) {
    match start_name(element) {
        "UpdateInfo" => {
            if let Some(done) = identity.take()
                && let Some(sealed) = seal_identity(done)
            {
                identities.push(sealed);
            }
            *update = None;
            *identity = Some(PartialIdentity::default());
        }
        "Update" if parent == Some("Updates") => {
            if let Some(done) = update.take()
                && let Some(sealed) = seal_update(done)
            {
                updates.push(sealed);
            }
            *identity = None;
            *update = Some(PartialUpdate::default());
        }
        _ => {}
    }
}

fn parse_sync_updates(response: &str) -> Result<Vec<Fe3File>, Fe3Error> {
    let escaped = escape_stray_ampersands(response);
    let mut reader = Reader::from_str(&escaped);
    reader.config_mut().trim_text(true);

    let mut identity: Option<PartialIdentity> = None;
    let mut update: Option<PartialUpdate> = None;
    // Document order preserved for deterministic results.
    let mut identities: Vec<(String, (String, String))> = Vec::new();
    let mut updates: Vec<(String, PartialUpdate)> = Vec::new();
    let mut id_capture = TextCapture::default();
    // Collects the inner XML of a record's `Xml` node while active; record
    // halves are only filled from its content. See `XmlCollector`.
    let mut xml = XmlCollector::default();
    let mut path: Vec<String> = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let parent = path.last().map(String::as_str);
                let name = start_name(&element);
                if xml.active {
                    // Inner content of a record's `Xml`: re-serialized
                    // verbatim while collecting (see `XmlCollector`).
                    xml.feed_start(&element, false);
                } else if name == "Xml" && (identity.is_some() || update.is_some()) {
                    // A record's `Xml` node starts the inner-XML collection.
                    xml.begin();
                } else {
                    if name == "ID" && matches!(parent, Some("UpdateInfo") | Some("Update")) {
                        id_capture.begin();
                    }
                    handle_element(
                        &element,
                        parent,
                        &mut identity,
                        &mut update,
                        &mut identities,
                        &mut updates,
                    );
                }
                path.push(name.to_owned());
            }
            Event::Empty(element) => {
                // Self-closing entries carry their attributes on `Empty` and
                // must not grow the parent path.
                if xml.active {
                    xml.feed_start(&element, true);
                } else {
                    let parent = path.last().map(String::as_str);
                    handle_element(
                        &element,
                        parent,
                        &mut identity,
                        &mut update,
                        &mut identities,
                        &mut updates,
                    );
                }
            }
            Event::Text(text) => {
                if xml.active {
                    // Kept raw (still escaped); the collector normalizes.
                    xml.feed_text(&text);
                } else {
                    id_capture.feed(&text)?;
                }
            }
            Event::GeneralRef(reference) => {
                if xml.active {
                    // Restored to `&name;` form, so reassembly matches the
                    // raw text chunks surrounding it.
                    xml.feed_reference(&reference);
                } else {
                    id_capture.feed_reference(&reference)?;
                }
            }
            Event::CData(text) => {
                if xml.active {
                    // Collects as markup; the sub-parse then sees balanced
                    // CDATA (never observed in live captures).
                    xml.saw_element = true;
                    xml.raw.push_str("<![CDATA[");
                    xml.raw.push_str(text.as_ref());
                    xml.raw.push_str("]]>");
                } else if id_capture.active {
                    id_capture.content.push_str(text.as_ref().trim());
                }
            }
            Event::End(element) => {
                let closing = end_name(&element);
                if xml.active && closing == "Xml" && xml.depth == 0 {
                    // A record's `Xml` closed: its collected content is
                    // normalized and sub-parsed into the pending half; the
                    // record's own end still seals it (arms below).
                    if let Some(inner) = xml.finish()? {
                        if let Some(info) = identity.as_mut() {
                            let parsed = parse_inner_identity(&inner)?;
                            info.update_id = parsed.update_id;
                            info.revision_number = parsed.revision_number;
                            info.secured = parsed.secured;
                        } else if let Some(pending) = update.as_mut() {
                            let (identity_name, files) = parse_inner_files(&inner)?;
                            pending.identity_name = identity_name;
                            pending.files = files;
                        }
                    }
                } else if xml.active {
                    // An inner element closes inside the collected content.
                    xml.close(closing);
                } else {
                    // The closed element itself is still on the path; the
                    // parent is one below it.
                    let parent = if path.len() >= 2 {
                        Some(path[path.len() - 2].as_str())
                    } else {
                        None
                    };
                    match closing {
                        "ID" => {
                            if let Some(id) = id_capture.finish() {
                                match parent {
                                    Some("UpdateInfo") => {
                                        if let Some(info) = identity.as_mut()
                                            && info.id.is_none()
                                        {
                                            info.id = Some(id);
                                        }
                                    }
                                    Some("Update") => {
                                        if let Some(pending) = update.as_mut()
                                            && pending.id.is_none()
                                        {
                                            pending.id = Some(id);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "UpdateInfo" => {
                            if let Some(done) = identity.take()
                                && let Some(sealed) = seal_identity(done)
                            {
                                identities.push(sealed);
                            }
                        }
                        "Update" if parent == Some("Updates") => {
                            if let Some(done) = update.take()
                                && let Some(sealed) = seal_update(done)
                            {
                                updates.push(sealed);
                            }
                        }
                        _ => {}
                    }
                }
                id_capture.stop();
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // Join the two halves by FE3 update id, in update-identity document
    // order. Entries missing either half are skipped silently: FE3 reports
    // plenty of non-secured companion updates.
    let mut files = Vec::new();
    for (id, (update_id, revision_number)) in identities {
        let Some((_, pending)) = updates.iter().find(|(pending_id, _)| *pending_id == id) else {
            continue;
        };
        let Some(identity_name) = pending.identity_name.clone() else {
            continue;
        };
        // The payload row is the one carrying `InstallerSpecificIdentifier`;
        // the dynamic-metadata companion (`Abm_*.cab`) trails the payload in
        // every live capture and is excluded by that.
        let Some(row) = pending
            .files
            .iter()
            .find(|row| row.installer_specific_identifier.is_some())
        else {
            continue;
        };
        let Some((
            file_name,
            installer_specific_identifier,
            size,
            digest,
            digest_algorithm,
            modified,
        )) = row.complete()
        else {
            continue;
        };
        files.push(Fe3File {
            file_name,
            installer_specific_identifier,
            package_identity_name: identity_name,
            update_id,
            revision_number,
            size,
            digest,
            digest_algorithm,
            modified,
        });
    }
    Ok(files)
}

/// Parses one `File` row; rows without an `InstallerSpecificIdentifier` are
/// not package payloads. `Size` that fails to parse (or is absent) yields no
/// row — the joined entry is dropped instead of guessed.
fn file_row(element: &BytesStart<'_>) -> Option<PartialFileRow> {
    let mut row = PartialFileRow::default();
    for attr in element.attributes() {
        let Ok(attr) = attr else {
            return None;
        };
        if file_attr(&attr, &mut row).is_err() {
            return None;
        }
    }
    row.installer_specific_identifier.as_ref()?;
    Some(row)
}

/// The identity-bearing fields of one `UpdateInfo`, extracted from the inner
/// XML of its `Xml` node (either wire form; see `XmlCollector`).
#[derive(Default)]
struct InnerIdentity {
    update_id: Option<String>,
    revision_number: Option<String>,
    secured: bool,
}

/// Applies an inner-XML `UpdateIdentity`/`SecuredFragment` element to the
/// parsed state; used for both the start and the (typical) self-closing form.
///
/// The `Prerequisites` decoy update identities nest under `AtLeastOne` and
/// only appear after the record's identity in document order, so the first
/// `UpdateIdentity` is the record's.
fn inner_identity_element(
    element: &BytesStart<'_>,
    parent: Option<&str>,
    parsed: &mut InnerIdentity,
) {
    match start_name(element) {
        "UpdateIdentity" if parsed.update_id.is_none() => {
            parsed.update_id = element_attr(element, "UpdateID");
            parsed.revision_number = element_attr(element, "RevisionNumber");
        }
        "SecuredFragment" if parent == Some("Properties") => parsed.secured = true,
        _ => {}
    }
}

/// Sub-parses the inner XML of an `UpdateInfo` record's `Xml` node.
fn parse_inner_identity(inner: &str) -> Result<InnerIdentity, Fe3Error> {
    let mut reader = Reader::from_str(inner);
    reader.config_mut().trim_text(true);

    let mut parsed = InnerIdentity::default();
    let mut path: Vec<String> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let parent = path.last().map(String::as_str);
                inner_identity_element(&element, parent, &mut parsed);
                path.push(start_name(&element).to_owned());
            }
            Event::Empty(element) => {
                let parent = path.last().map(String::as_str);
                inner_identity_element(&element, parent, &mut parsed);
            }
            Event::Eof => return Ok(parsed),
            _ => {}
        }
    }
}

/// Applies an inner-XML `ExtendedProperties`/`File` element to the parsed
/// state; used for both the start and the (typical) self-closing form.
fn inner_files_element(
    element: &BytesStart<'_>,
    parent: Option<&str>,
    identity_name: &mut Option<String>,
    rows: &mut Vec<PartialFileRow>,
) {
    match start_name(element) {
        "ExtendedProperties" if identity_name.is_none() => {
            *identity_name = element_attr(element, "PackageIdentityName");
        }
        "File" if parent == Some("Files") => {
            if let Some(row) = file_row(element) {
                rows.push(row);
            }
        }
        _ => {}
    }
}

/// Sub-parses the inner XML of an `Update` record's `Xml` node into the
/// package identity name and its file rows.
fn parse_inner_files(inner: &str) -> Result<(Option<String>, Vec<PartialFileRow>), Fe3Error> {
    let mut reader = Reader::from_str(inner);
    reader.config_mut().trim_text(true);

    let mut identity_name: Option<String> = None;
    let mut rows: Vec<PartialFileRow> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let parent = path.last().map(String::as_str);
                inner_files_element(&element, parent, &mut identity_name, &mut rows);
                path.push(start_name(&element).to_owned());
            }
            Event::Empty(element) => {
                let parent = path.last().map(String::as_str);
                inner_files_element(&element, parent, &mut identity_name, &mut rows);
            }
            Event::Eof => return Ok((identity_name, rows)),
            _ => {}
        }
    }
}

/// Collects every URL under `FileLocation` elements of a `secured` response.
/// The response is NOT well-formed XML (raw `&` in tokenized URLs); see
/// [`escape_stray_ampersands`].
fn parse_file_locations(response: &str) -> Result<Vec<String>, Fe3Error> {
    let escaped = escape_stray_ampersands(response);
    let mut reader = Reader::from_str(&escaped);
    reader.config_mut().trim_text(true);

    let mut urls: Vec<String> = Vec::new();
    let mut capture = TextCapture::default();
    let mut path: Vec<String> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let parent = path.last().map(String::as_str);
                let name = start_name(&element);
                if parent == Some("FileLocation") && name == "Url" {
                    capture.begin();
                }
                path.push(name.to_owned());
            }
            Event::Text(text) => capture.feed(&text)?,
            Event::GeneralRef(reference) => capture.feed_reference(&reference)?,
            Event::CData(text) => {
                if capture.active {
                    capture.content.push_str(text.as_ref().trim());
                }
            }
            Event::End(element) => {
                if end_name(&element) == "Url"
                    && capture.active
                    && let Some(url) = capture.finish()
                {
                    urls.push(url);
                }
                capture.stop();
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_template_replaces_all_occurrences() {
        let filled = fill_template(
            "a{0}b{1}c{2}d{2}",
            &[("{0}", "cookie"), ("{1}", "category"), ("{2}", "Retail")],
        );
        assert_eq!(filled, "acookiebcategorycRetaildRetail");
    }

    #[test]
    fn templates_carry_expected_placeholders() {
        assert!(FE3_GET_COOKIE_TEMPLATE.contains("GetCookie"));
        assert!(FE3_SYNC_UPDATES_TEMPLATE.contains("{0}"));
        assert!(FE3_SYNC_UPDATES_TEMPLATE.contains("{1}"));
        assert!(FE3_SYNC_UPDATES_TEMPLATE.contains("{2}"));
        assert!(FE3_FILE_URL_TEMPLATE.contains("{0}"));
        assert!(FE3_FILE_URL_TEMPLATE.contains("{1}"));
        assert!(FE3_FILE_URL_TEMPLATE.contains("{2}"));
    }

    #[test]
    fn escape_stray_ampersands_escapes_only_stray() {
        let input = "?P1=1791792214&P2=404&P3=2&P4=abc%2f";
        let escaped = escape_stray_ampersands(input);
        assert_eq!(escaped, "?P1=1791792214&amp;P2=404&amp;P3=2&amp;P4=abc%2f");
        // Idempotent.
        assert_eq!(escape_stray_ampersands(&escaped), escaped);
        // Well-formed references survive.
        assert_eq!(
            escape_stray_ampersands("a&amp;b&#65;c&"),
            "a&amp;b&#65;c&amp;"
        );
    }

    #[test]
    fn parse_cookie_returns_escaped_content_unescaped() {
        let response = r#"<Envelope><Header><Security
            xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd"
        ><EncryptedData>pIRBCABZ&amp;co+ok/ie==</EncryptedData></Security></Header><Body></Body></Envelope>"#;
        let cookie = parse_cookie(response).unwrap();
        assert_eq!(cookie, "pIRBCABZ&co+ok/ie==");
    }

    #[test]
    fn parse_cookie_rejects_missing_or_empty() {
        assert!(parse_cookie("<Envelope><Body/></Envelope>").is_err());
        assert!(
            parse_cookie("<Envelope><Body><EncryptedData>  </EncryptedData></Body></Envelope>")
                .is_err()
        );
    }

    /// Mirrors the live `SyncUpdates` shape (2026-10-12): SOAP prefixes, the
    /// two parallel halves joined by numeric id, and all decoys (`Deployment`
    /// id, `Prerequisites` update identities, non-secured entries, metadata
    /// file rows without `InstallerSpecificIdentifier`). Inner `Xml` content
    /// arrives here in the real-elements wire form (`Xml` child elements).
    const SYNC_UPDATES_SAMPLE: &str = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope">
      <s:Body>
        <SyncUpdatesResponse xmlns="http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService">
          <SyncUpdatesResult>
            <NewUpdates>
              <UpdateInfo>
                <ID>329883204</ID>
                <Deployment>
                  <ID>586609816</ID>
                </Deployment>
                <Xml>
                  <UpdateIdentity UpdateID="05679a73-0257-4f1d-9205-2357cdd261cb" RevisionNumber="1"/>
                  <Properties UpdateType="Software" PerUser="true" IsAppxFramework="true" PackageRank="100">
                    <SecuredFragment>FileUrl</SecuredFragment>
                  </Properties>
                </Xml>
              </UpdateInfo>
              <UpdateInfo>
                <ID>111222333</ID>
                <Xml>
                  <UpdateIdentity UpdateID="2d1560f1-7c3b-4b0a-a487-c0119311ab67" RevisionNumber="4"/>
                  <Properties>
                    <SecuredFragment>FileUrl</SecuredFragment>
                  </Properties>
                </Xml>
              </UpdateInfo>
              <UpdateInfo>
                <ID>444555666</ID>
                <Xml>
                  <UpdateIdentity UpdateID="cccccccc-0000-0000-0000-000000000000" RevisionNumber="7"/>
                  <Properties>
                  </Properties>
                </Xml>
              </UpdateInfo>
              <UpdateInfo>
                <ID>777888999</ID>
                <Xml>
                  <Properties>
                    <SecuredFragment>FileUrl</SecuredFragment>
                  </Properties>
                </Xml>
              </UpdateInfo>
            </NewUpdates>
            <ExtendedUpdateInfo>
              <Updates>
                <Update>
                  <ID>329883204</ID>
                  <Xml>
                    <ExtendedProperties Handler="http://schemas.microsoft.com/msus/2002/12/UpdateHandlers/AppxPackage"
                      PackageIdentityName="5319275A.WhatsAppDesktop"
                      PackageContentId="7152871f-a10c-5214-9d97-97495e3d2185"/>
                    <Files>
                      <File FileName="473405aa-5d34-4e7e-b8e2-8afc194a5e66.msixbundle"
                        InstallerSpecificIdentifier="5319275A.WhatsAppDesktop_2.2639.100.0_neutral_~_cv1g1gvanyjgm"
                        Digest="r1nvfOM0tbDxp96Uupg+tg+I/Ig=" DigestAlgorithm="SHA1"
                        Size="311793167" Modified="2026-10-07T15:13:29.1599172Z"/>
                      <File FileName="Abm_00000000-0000-0000-0000-000000000000.cab"
                        Digest="YUJJQk5Ceg==" DigestAlgorithm="SHA1" Size="375517"
                        Modified="2026-10-07T15:13:29.1599172Z" PatchingType="DynamicMetadata"/>
                    </Files>
                    <HandlerSpecificData type="appx:AppxInstaller">
                      <AppxPackageInstallData/>
                    </HandlerSpecificData>
                  </Xml>
                </Update>
                <Update>
                  <ID>999888777</ID>
                  <Xml>
                    <ExtendedProperties PackageIdentityName="NoFiles.Package"/>
                  </Xml>
                </Update>
                <Update>
                  <ID>111222333</ID>
                  <Xml>
                    <ExtendedProperties PackageIdentityName="5319275A.VCLibs"/>
                    <Files>
                      <File FileName="ddddddep-msix" InstallerSpecificIdentifier="5319275A.VCLibs_14.0.33519.0_x86__cv1g1gvanyjgm"
                        Digest="ZGVwbWFjaA==" DigestAlgorithm="SHA1" Size="758544"
                        Modified="2026-10-07T15:13:29.1599172Z"/>
                    </Files>
                  </Xml>
                </Update>
              </Updates>
            </ExtendedUpdateInfo>
          </SyncUpdatesResult>
        </SyncUpdatesResponse>
      </s:Body>
    </s:Envelope>"#;

    #[test]
    fn parse_sync_updates_joins_and_skips_decoys() {
        let files = parse_sync_updates(SYNC_UPDATES_SAMPLE).unwrap();

        // UpdateInfo 444555666 (no SecuredFragment) and 777888999 (no
        // UpdateIdentity) are excluded; update 999888777 (no Files) is not
        // joined. Document order of the identity half governs.
        assert_eq!(files.len(), 2);
        let main = &files[0];
        assert_eq!(main.update_id, "05679a73-0257-4f1d-9205-2357cdd261cb");
        assert_eq!(main.revision_number, "1");
        assert_eq!(main.package_identity_name, "5319275A.WhatsAppDesktop");
        assert_eq!(
            main.installer_specific_identifier,
            "5319275A.WhatsAppDesktop_2.2639.100.0_neutral_~_cv1g1gvanyjgm"
        );
        assert_eq!(
            main.file_name,
            "473405aa-5d34-4e7e-b8e2-8afc194a5e66.msixbundle"
        );
        assert_eq!(main.size, 311793167);
        assert_eq!(main.digest, "r1nvfOM0tbDxp96Uupg+tg+I/Ig=");
        assert_eq!(main.digest_algorithm, "SHA1");
        assert_eq!(main.modified, "2026-10-07T15:13:29.1599172Z");

        let dep = &files[1];
        assert_eq!(dep.update_id, "2d1560f1-7c3b-4b0a-a487-c0119311ab67");
        assert_eq!(dep.package_identity_name, "5319275A.VCLibs");
        assert_eq!(dep.size, 758544);
    }

    /// Verbatim capture (2026-10-12,
    /// `scratches/fe3_spike/transcripts/6_fe3_fileurl_response_MAINBUNDLE.xml`),
    /// invalid XML as delivered: raw unescaped `&` inside the tokenized URL.
    const SECURED_RESPONSE_SAMPLE: &str = r#"<Envelope
        xmlns="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://www.w3.org/2005/08/addressing">
      <Body>
        <GetExtendedUpdateInfo2Response xmlns="http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService">
          <GetExtendedUpdateInfo2Result>
            <FileLocations>
              <FileLocation>
                <FileDigest>G+7b31lE67w9deIV9gOfHphdESc=</FileDigest>
                <Url>http://dl.delivery.mp.microsoft.com/filestreamingservice/files/e8542dcd-47c3-44cb-af25-19f39dbb4f07</Url>
              </FileLocation>
              <FileLocation>
                <FileDigest>BrW83LeHL3WtU9/MkkK5m8Rjw7k=</FileDigest>
                <Url>http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/7fd9017e-c11e-476b-9266-b870073f0914?P1=1791792214&P2=404&P3=2&P4=JtblvlI02aUVQM2I0hXQG%2fR578mFIh0nhMM2Yi0ZEEtz777VuIvgh%2bE2O7AcRBq6bOPPpOwQ5Q2hEgDiOSvNzA%3d%3d</Url>
              </FileLocation>
            </FileLocations>
          </GetExtendedUpdateInfo2Result>
        </GetExtendedUpdateInfo2Response>
      </Body>
    </Envelope>"#;

    #[test]
    fn parse_file_locations_survives_raw_ampersands() {
        let urls = parse_file_locations(SECURED_RESPONSE_SAMPLE).unwrap();
        assert_eq!(urls.len(), 2);
        // Exactly the blockmap shape: 99 characters, fixed host, no query.
        assert_eq!(urls[0].len(), SECURED_BLOCKMAP_URL_LEN);
        assert!(urls[0].starts_with("http://dl.delivery.mp.microsoft.com/"));
        // The payload URL arrives tokenized, with raw `&` restored verbatim.
        assert!(urls[1].starts_with("http://tlu.dl.delivery.mp.microsoft.com/"));
        assert!(urls[1].contains("?P1=1791792214&P2=404&P3=2&P4="));
    }

    #[test]
    fn parse_file_locations_tolerates_missing_urls() {
        let urls = parse_file_locations(
            "<Envelope><Body><GetExtendedUpdateInfo2Response><FileLocations></FileLocations></GetExtendedUpdateInfo2Response></Body></Envelope>",
        )
        .unwrap();
        assert!(urls.is_empty());
    }

    /// Verbatim live capture pair (2026-10-12), escaped-text wire form of the
    /// inner `Xml` (`scratches/fe3_spike/zone_info_329883204.txt` +
    /// `zone_update_329883204.txt`, one record per half, envelope wrapper like
    /// in live responses). The same numeric id (329883204) joins both halves.
    /// Decoys preserved from the capture: `Deployment` id, `Prerequisites`
    /// update identities, and a second `File` row without
    /// `InstallerSpecificIdentifier` (`Abm_*.cab`).
    const SYNC_UPDATES_SAMPLE_TEXT_ESCAPED: &str = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope">
  <s:Body>
    <SyncUpdatesResponse xmlns="http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService">
      <SyncUpdatesResult>
        <NewUpdates>
<UpdateInfo><ID>329883204</ID><Deployment><ID>586609816</ID><Action>Bundle</Action><IsAssigned>true</IsAssigned><LastChangeTime>2026-06-09</LastChangeTime></Deployment><IsLeaf>true</IsLeaf><Xml>&lt;UpdateIdentity UpdateID="05679a73-0257-4f1d-9205-2357cdd261cb" RevisionNumber="1" /&gt;&lt;Properties UpdateType="Software" PerUser="true" IsAppxFramework="true" PackageRank="100"&gt;&lt;SecuredFragment&gt;FileUrl&lt;/SecuredFragment&gt;&lt;/Properties&gt;&lt;Relationships&gt;&lt;Prerequisites&gt;&lt;AtLeastOne&gt;&lt;UpdateIdentity UpdateID="c746f7a0-3385-4c03-b4fa-7da2943db294" /&gt;&lt;/AtLeastOne&gt;&lt;UpdateIdentity UpdateID="31ec4b55-3ef0-4a75-aca2-18c7180637b7" /&gt;&lt;AtLeastOne&gt;&lt;UpdateIdentity UpdateID="3d854509-0f65-46d6-b804-83b3e28c47e0" /&gt;&lt;UpdateIdentity UpdateID="25ce6bbd-894d-486a-b7d2-1bd23f192cfe" /&gt;&lt;/AtLeastOne&gt;&lt;/Prerequisites&gt;&lt;/Relationships&gt;&lt;ApplicabilityRules&gt;&lt;IsInstalled&gt;&lt;AppxPackageInstalled /&gt;&lt;/IsInstalled&gt;&lt;IsInstallable&gt;&lt;AppxPackageInstallable /&gt;&lt;/IsInstallable&gt;&lt;Metadata&gt;&lt;AppxPackageMetadata&gt;&lt;AppxMetadata PackageType="UAP" IsAppxBundle="false" PackageMoniker="Microsoft.VCLibs.140.00_14.0.33519.0_arm__8wekyb3d8bbwe"&gt;&lt;ApplicabilityBlob&gt;{"blob.version":1688867040526336,"content.isFramework":true,"content.isMain":false,"content.packageId":"Microsoft.VCLibs.140.00_14.0.33519.0_arm__8wekyb3d8bbwe","content.productId":"938c94d3-76d8-49d2-8524-dd6e4581ff3a","content.targetPlatforms":[{"platform.maxVersionTested":2814750425219072,"platform.minVersion":2814750425219072,"platform.target":0}],"content.type":7,"policy":{"category.first":"app","category.second":"Developer tools","category.third":"Development kits","optOut.backupRestore":false,"optOut.removeableMedia":false},"policy2":{"ageRating":1,"optOut.DVR":false,"thirdPartyAppRatings":[{"level":7,"systemId":3},{"level":12,"systemId":5},{"level":48,"systemId":12},{"level":27,"systemId":9},{"level":76,"systemId":16},{"level":68,"systemId":15},{"level":54,"systemId":13}]}}&lt;/ApplicabilityBlob&gt;&lt;/AppxMetadata&gt;&lt;/AppxPackageMetadata&gt;&lt;/Metadata&gt;&lt;/ApplicabilityRules&gt;</Xml></UpdateInfo>
        </NewUpdates>
        <ExtendedUpdateInfo>
          <Updates>
<Update><ID>329883204</ID><Xml>&lt;ExtendedProperties Handler="http://schemas.microsoft.com/msus/2002/12/UpdateHandlers/AppxPackage" IsAppxFramework="true" DefaultPropertiesLanguage="en" MaxDownloadSize="837201" MinDownloadSize="0" FromStoreService="true" CreationDate="2026-06-09T11:21:21.1414049Z" ContentType="Application" PackageIdentityName="Microsoft.VCLibs.140.00" PackageContentId="d5341d97-2f34-3aa0-c0d0-b693b8541db6"&gt;&lt;InstallationBehavior /&gt;&lt;/ExtendedProperties&gt;&lt;Files&gt;&lt;File FileName="f52e53fc-9664-4720-800e-a637f6f5c125.appx" Digest="QcvoJerwPZqVHhAet72YUZvX/eI=" DigestAlgorithm="SHA1" Size="835614" Modified="2024-01-29T20:03:05.7437236Z" InstallerSpecificIdentifier="Microsoft.VCLibs.140.00_14.0.33519.0_arm__8wekyb3d8bbwe"&gt;&lt;AdditionalDigest Algorithm="SHA256"&gt;LEIlI/9pNomoTBCVhc+kRBQ6w7a3pcrfSFivxqPLdQ8=&lt;/AdditionalDigest&gt;&lt;PiecesHashDigest Algorithm="SHA256"&gt;0jxxtMWYZUGx9PjKxcKfR8dVl1ZKXdkA/qFW3WPNv3k=&lt;/PiecesHashDigest&gt;&lt;BlockMapDigest Algorithm="SHA256"&gt;wFiAGZ3fuldahbUjns4+RksTqg9WgTi6v91vsDQBG+w=&lt;/BlockMapDigest&gt;&lt;/File&gt;&lt;File FileName="Abm_f52e53fc-9664-4720-800e-a637f6f5c125.cab" Digest="eZ9xNey/fo1yKnfdjbPUEVpn0JY=" DigestAlgorithm="SHA1" Size="1587" Modified="2024-01-29T20:03:05.7437236Z" PatchingType="DynamicMetadata"&gt;&lt;AdditionalDigest Algorithm="SHA256"&gt;wFiAGZ3fuldahbUjns4+RksTqg9WgTi6v91vsDQBG+w=&lt;/AdditionalDigest&gt;&lt;/File&gt;&lt;/Files&gt;&lt;HandlerSpecificData type="appx:AppxInstaller"&gt;&lt;AppxPackageInstallData PackageFileName="f52e53fc-9664-4720-800e-a637f6f5c125.appx" MainPackage="false" /&gt;&lt;/HandlerSpecificData&gt;</Xml></Update>
          </Updates>
        </ExtendedUpdateInfo>
      </SyncUpdatesResult>
    </SyncUpdatesResponse>
  </s:Body>
</s:Envelope>"#;

    #[test]
    fn parse_sync_updates_reads_escaped_text_xml_form() {
        let files = parse_sync_updates(SYNC_UPDATES_SAMPLE_TEXT_ESCAPED).unwrap();

        // Both halves joined from their escaped-text `Xml` content; the
        // first `UpdateIdentity` (05679a73..) wins over the `Prerequisites`
        // decoys; the `Abm_*.cab` row carries no `InstallerSpecificIdentifier`
        // and is dropped.
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.update_id, "05679a73-0257-4f1d-9205-2357cdd261cb");
        assert_eq!(file.revision_number, "1");
        assert_eq!(file.package_identity_name, "Microsoft.VCLibs.140.00");
        assert_eq!(
            file.installer_specific_identifier,
            "Microsoft.VCLibs.140.00_14.0.33519.0_arm__8wekyb3d8bbwe"
        );
        assert_eq!(file.file_name, "f52e53fc-9664-4720-800e-a637f6f5c125.appx");
        assert_eq!(file.size, 835614);
        assert_eq!(file.digest, "QcvoJerwPZqVHhAet72YUZvX/eI=");
        assert_eq!(file.digest_algorithm, "SHA1");
        assert_eq!(file.modified, "2024-01-29T20:03:05.7437236Z");
    }

    /// Decodes the embedded trust root, so a broken embed fails here instead
    /// of only at the first TLS connect.
    #[test]
    fn fe3_root_pem_decodes() {
        let _root = reqwest::Certificate::from_pem(FE3_ROOT_PEM.as_bytes()).unwrap();
    }
}
