//! XML models of Appx package and bundle manifests.
//!
//! Field names, nesting, and namespace behavior verified against live
//! containers (2026-10-12, `scratches/fe3_spike/downloads/`): the live
//! bundle manifest (`AppxMetadata/AppxBundleManifest.xml`,
//! `SchemaVersion="5.0"`) qualifies its `Dependencies` as `b4:` while a
//! package manifest (`AppxManifest.xml`) uses unprefixed `Dependencies`
//! and `Applications`. Elements and attributes are therefore matched by
//! local name, ignoring any prefix, and namespace declarations on the
//! way are simply skipped.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::MsixError;

/// `Package/Identity` of both manifest types.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestIdentity {
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub processor_architecture: Option<String>,
}

/// One `Package` entry of `AppxBundleManifest.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BundlePackage {
    /// `application` (payload) or `resource` (scale/language satellites).
    pub package_type: String,
    pub version: String,
    /// Resource packages carry a `ResourceId` instead and may lack any
    /// architecture (live bundle, 2026-10-12).
    pub architecture: Option<String>,
    /// Name of the inner package inside the bundle archive.
    pub file_name: String,
    /// Byte offset of the inner package in the bundle archive.
    pub offset: u64,
    /// Uncompressed length of the inner package in the bundle archive.
    pub size: u64,
}

/// Parsed `AppxMetadata/AppxBundleManifest.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BundleManifest {
    pub identity: ManifestIdentity,
    pub packages: Vec<BundlePackage>,
}

/// `Package/Dependencies/TargetDeviceFamily` of `AppxManifest.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetDeviceFamily {
    pub name: String,
    pub min_version: String,
    pub max_version_tested: String,
}

/// `Package/Dependencies/PackageDependency` of `AppxManifest.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackageDependency {
    pub name: String,
    pub min_version: String,
    pub publisher: Option<String>,
}

/// `Package/Applications/Application` of `AppxManifest.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Application {
    pub id: String,
    pub executable: String,
    pub entry_point: Option<String>,
}

/// One `Extensions/Extension/InProcessServer/ActivatableClass` of
/// `AppxManifest.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivatableClass {
    /// `ActivatableClass/@ActivatableClassId` — the WinRT class name.
    pub class_id: String,
    /// `InProcessServer/Path` — the server DLL, relative to the package
    /// root in Windows path spelling.
    pub server_path: String,
}

/// Parsed `AppxManifest.xml` — the install-relevant report fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackageManifest {
    pub identity: ManifestIdentity,
    pub target_device_families: Vec<TargetDeviceFamily>,
    pub package_dependencies: Vec<PackageDependency>,
    pub applications: Vec<Application>,
}

/// The application-type packages of a bundle for one architecture (e.g.
/// `x64`); resource packages (scale/language satellites) never match.
pub fn select_arch_application_packages(
    manifest: &BundleManifest,
    arch: &str,
) -> Vec<BundlePackage> {
    manifest
        .packages
        .iter()
        .filter(|package| {
            package.package_type.eq_ignore_ascii_case("application")
                && package
                    .architecture
                    .as_deref()
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(arch))
        })
        .cloned()
        .collect()
}

/// Parses `AppxMetadata/AppxBundleManifest.xml`.
pub fn parse_bundle_manifest(xml: &str) -> Result<BundleManifest, MsixError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut manifest = BundleManifest::default();
    let mut stack: Vec<String> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                visit_bundle_element(&mut manifest, stack.last().map(String::as_str), &start)?;
                stack.push(local_name(&start));
            }
            Event::Empty(start) => {
                // Self-closing elements carry attributes but produce no
                // End event and must not land on the element stack.
                visit_bundle_element(&mut manifest, stack.last().map(String::as_str), &start)?;
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {
                // Comments, text, declarations: irrelevant shape.
            }
        }
    }
    Ok(manifest)
}

/// Parses `AppxManifest.xml` into the install-relevant fields.
pub fn parse_package_manifest(xml: &str) -> Result<PackageManifest, MsixError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut manifest = PackageManifest::default();
    let mut stack: Vec<String> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                visit_package_element(&mut manifest, stack.last().map(String::as_str), &start)?;
                stack.push(local_name(&start));
            }
            Event::Empty(start) => {
                visit_package_element(&mut manifest, stack.last().map(String::as_str), &start)?;
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(manifest)
}

/// Parses the WinRT activatable classes declared by `AppxManifest.xml`:
/// every `Extensions/Extension[@Category="windows.activatableClass.inProcessServer"]/
/// InProcessServer` with its server `Path` and one entry per
/// `ActivatableClass`.
///
/// This is the data `store run` seeds into the wine registry (wine's
/// combase resolves RoGetActivationFactory names that are not builtin
/// factories through `HKLM\Software\Microsoft\WindowsRuntime\
/// ActivatableClassId\<Class>` @ `DllPath`).
pub fn parse_activatable_classes(xml: &str) -> Result<Vec<ActivatableClass>, MsixError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut classes = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    // Manifests list the server `Path` inside `InProcessServer` before
    // its `ActivatableClass` entries, so one slot per server suffices.
    let mut current_server = String::new();
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                let local = local_name(&start);
                match local.as_str() {
                    "InProcessServer" => current_server.clear(),
                    "Path" if stack.last().map(String::as_str) == Some("InProcessServer") => {}
                    "ActivatableClass"
                        if stack.last().map(String::as_str) == Some("InProcessServer") =>
                    {
                        push_activatable_class(&start, &current_server, &mut classes)?
                    }
                    _ => {}
                }
                stack.push(local);
            }
            Event::Empty(start) => {
                let local = local_name(&start);
                if local == "ActivatableClass"
                    && stack.last().map(String::as_str) == Some("InProcessServer")
                {
                    push_activatable_class(&start, &current_server, &mut classes)?
                }
            }
            Event::Text(text) => {
                if stack.last().map(String::as_str) == Some("Path") {
                    current_server = text.xml_content(XmlVersion::Implicit1_0).into_owned();
                }
            }
            Event::End(_) => {
                if stack.last().map(String::as_str) == Some("InProcessServer") {
                    current_server.clear();
                }
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(classes)
}

fn push_activatable_class(
    start: &BytesStart,
    server_path: &str,
    classes: &mut Vec<ActivatableClass>,
) -> Result<(), MsixError> {
    let Some(class_id) = attribute(start, "ActivatableClassId")? else {
        // No id, nothing to activate: skip silently like the other
        // optional attributes.
        return Ok(());
    };
    classes.push(ActivatableClass {
        class_id,
        server_path: server_path.to_owned(),
    });
    Ok(())
}

fn visit_bundle_element(
    manifest: &mut BundleManifest,
    parent: Option<&str>,
    start: &BytesStart,
) -> Result<(), MsixError> {
    let local = local_name(start);
    match (parent, local.as_str()) {
        (Some("Bundle"), "Identity") => manifest.identity = parse_identity(start)?,
        (Some("Packages"), "Package") => manifest.packages.push(parse_bundle_package(start)?),
        _ => {}
    }
    Ok(())
}

fn visit_package_element(
    manifest: &mut PackageManifest,
    parent: Option<&str>,
    start: &BytesStart,
) -> Result<(), MsixError> {
    let local = local_name(start);
    match (parent, local.as_str()) {
        (Some("Package"), "Identity") => manifest.identity = parse_identity(start)?,
        (Some("Dependencies"), "TargetDeviceFamily") => manifest
            .target_device_families
            .push(parse_target_device_family(start)?),
        (Some("Dependencies"), "PackageDependency") => manifest
            .package_dependencies
            .push(parse_package_dependency(start)?),
        (Some("Applications"), "Application") => {
            manifest.applications.push(parse_application(start)?)
        }
        _ => {}
    }
    Ok(())
}

fn local_name(start: &BytesStart) -> String {
    start.name().local_name().as_ref().to_owned()
}

fn attribute(element: &BytesStart, name: &str) -> Result<Option<String>, MsixError> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if attribute.key.local_name().as_ref() == name {
            let value = attribute.normalized_value(XmlVersion::Implicit1_0)?;
            return Ok(Some(value.to_string()));
        }
    }
    Ok(None)
}

fn required_attribute(
    element: &BytesStart,
    element_name: &str,
    name: &str,
) -> Result<String, MsixError> {
    attribute(element, name)?.ok_or_else(|| MsixError::MissingAttribute {
        element: element_name.to_owned(),
        attribute: name.to_owned(),
    })
}

fn number_attribute(
    element: &BytesStart,
    element_name: &str,
    name: &str,
) -> Result<Option<u64>, MsixError> {
    match attribute(element, name)? {
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|_| MsixError::BadNumberAttribute {
                element: element_name.to_owned(),
                attribute: name.to_owned(),
                value,
            }),
        None => Ok(None),
    }
}

fn parse_identity(element: &BytesStart) -> Result<ManifestIdentity, MsixError> {
    Ok(ManifestIdentity {
        name: required_attribute(element, "Identity", "Name")?,
        publisher: required_attribute(element, "Identity", "Publisher")?,
        version: required_attribute(element, "Identity", "Version")?,
        processor_architecture: attribute(element, "ProcessorArchitecture")?,
    })
}

fn parse_bundle_package(element: &BytesStart) -> Result<BundlePackage, MsixError> {
    Ok(BundlePackage {
        package_type: required_attribute(element, "Package", "Type")?,
        version: required_attribute(element, "Package", "Version")?,
        architecture: attribute(element, "Architecture")?,
        file_name: required_attribute(element, "Package", "FileName")?,
        offset: number_attribute(element, "Package", "Offset")?.unwrap_or(0),
        size: number_attribute(element, "Package", "Size")?.unwrap_or(0),
    })
}

fn parse_target_device_family(element: &BytesStart) -> Result<TargetDeviceFamily, MsixError> {
    Ok(TargetDeviceFamily {
        name: required_attribute(element, "TargetDeviceFamily", "Name")?,
        min_version: required_attribute(element, "TargetDeviceFamily", "MinVersion")?,
        max_version_tested: required_attribute(element, "TargetDeviceFamily", "MaxVersionTested")?,
    })
}

fn parse_package_dependency(element: &BytesStart) -> Result<PackageDependency, MsixError> {
    Ok(PackageDependency {
        name: required_attribute(element, "PackageDependency", "Name")?,
        min_version: required_attribute(element, "PackageDependency", "MinVersion")?,
        publisher: attribute(element, "Publisher")?,
    })
}

fn parse_application(element: &BytesStart) -> Result<Application, MsixError> {
    Ok(Application {
        id: required_attribute(element, "Application", "Id")?,
        executable: required_attribute(element, "Application", "Executable")?,
        entry_point: attribute(element, "EntryPoint")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live bundle manifest (2026-10-12, downloads/473405aa….msixbundle),
    /// trimmed to the identity and three of the 13 package entries; the
    /// live manifest qualifies `Dependencies` as `b4:`.
    const BUNDLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<Bundle SchemaVersion="5.0" IgnorableNamespaces="b4 b5" xmlns="http://schemas.microsoft.com/appx/2013/bundle" xmlns:b4="http://schemas.microsoft.com/appx/2018/bundle" xmlns:b5="http://schemas.microsoft.com/appx/2019/bundle">
	<Identity Name="5319275A.WhatsAppDesktop" Publisher="CN=24803D75-212C-471A-BC57-9EF86AB91435" Version="2.2639.100.0"/>
	<Packages>
		<Package Type="application" Version="2.2639.100.0" Architecture="arm64" FileName="WhatsApp.Root_2.2639.100.0_arm64.msix" Offset="67" Size="151601887">
			<Resources><Resource Language="EN"/></Resources>
			<b4:Dependencies>
				<b4:TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.19041.0"/>
			</b4:Dependencies>
		</Package>
		<Package Type="application" Version="2.2639.100.0" Architecture="x64" FileName="WhatsApp.Root_2.2639.100.0_x64.msix" Offset="155165703" Size="156616001"/>
		<Package Type="resource" Version="2.2639.100.0" ResourceId="split.scale-100" FileName="WhatsApp.Root_2.2639.100.0_scale-100.msix" Offset="151602049" Size="215790"/>
	</Packages>
</Bundle>"#;

    /// Live package manifest (from the extracted WhatsApp x64 reference
    /// package), trimmed to identity, dependencies and the application
    /// entry; starts with a byte order mark like the live file.
    const PACKAGE_XML: &str = concat!(
        "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
        "<Package xmlns=\"http://schemas.microsoft.com/appx/manifest/foundation/windows10\" \
         xmlns:mp=\"http://schemas.microsoft.com/appx/2014/phone/manifest\" \
         xmlns:rescap=\"http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities\" \
         xmlns:uap=\"http://schemas.microsoft.com/appx/manifest/uap/windows10\" IgnorableNamespaces=\"uap rescap\">\n",
        "  <Identity Name=\"5319275A.WhatsAppDesktop\" Publisher=\"CN=24803D75-212C-471A-BC57-9EF86AB91435\" Version=\"2.2587.9.0\" ProcessorArchitecture=\"x64\" />\n",
        "  <mp:PhoneIdentity PhoneProductId=\"606be18e-7eed-4c2c-b480-6211dd1826d6\" />\n",
        "  <Properties><DisplayName>WhatsApp</DisplayName></Properties>\n",
        "  <Dependencies>\n",
        "    <TargetDeviceFamily Name=\"Windows.Universal\" MinVersion=\"10.0.17763.0\" MaxVersionTested=\"10.0.19041.0\" />\n",
        "    <TargetDeviceFamily Name=\"Windows.Desktop\" MinVersion=\"10.0.17763.0\" MaxVersionTested=\"10.0.19041.0\" />\n",
        "    <PackageDependency Name=\"Microsoft.WindowsAppRuntime.1.6\" MinVersion=\"6000.318.2304.0\" Publisher=\"CN=Microsoft Corporation\" />\n",
        "    <PackageDependency Name=\"Microsoft.VCLibs.140.00\" MinVersion=\"14.0.33519.0\" />\n",
        "    <PackageDependency Name=\"Microsoft.VCLibs.140.00.UWPDesktop\" MinVersion=\"14.0.33728.0\" />\n",
        "  </Dependencies>\n",
        "  <Applications>\n",
        "    <Application Id=\"WhatsApp.Root\" Executable=\"WhatsApp.Root.exe\" EntryPoint=\"Windows.FullTrustApplication\">\n",
        "      <uap:VisualElements DisplayName=\"WhatsApp\" Square150x150Logo=\"Assets\\Logo.png\" />\n",
        "    </Application>\n",
        "  </Applications>\n",
        "  <Capabilities><rescap:Capability Name=\"runFullTrust\" /></Capabilities>\n",
        "</Package>\n",
    );

    #[test]
    fn bundle_manifest_reads_identity_and_packages() {
        let manifest = parse_bundle_manifest(BUNDLE_XML).unwrap();
        assert_eq!(manifest.identity.name, "5319275A.WhatsAppDesktop");
        assert_eq!(manifest.identity.version, "2.2639.100.0");
        assert_eq!(
            manifest.identity.publisher,
            "CN=24803D75-212C-471A-BC57-9EF86AB91435"
        );
        assert_eq!(manifest.identity.processor_architecture, None);
        assert_eq!(manifest.packages.len(), 3);
        assert_eq!(manifest.packages[0].package_type, "application");
        assert_eq!(manifest.packages[0].architecture.as_deref(), Some("arm64"));
        assert_eq!(
            manifest.packages[0].file_name,
            "WhatsApp.Root_2.2639.100.0_arm64.msix"
        );
        assert_eq!(manifest.packages[0].offset, 67);
        assert_eq!(manifest.packages[0].size, 151601887);
        assert_eq!(manifest.packages[1].architecture.as_deref(), Some("x64"));
        assert_eq!(manifest.packages[2].package_type, "resource");
        // Resource packages carry a ResourceId and no architecture.
        assert_eq!(manifest.packages[2].architecture, None);
        assert_eq!(manifest.packages[2].offset, 151602049);
    }

    #[test]
    fn arch_selection_picks_application_packages_for_the_arch() {
        let manifest = parse_bundle_manifest(BUNDLE_XML).unwrap();
        let x64 = select_arch_application_packages(&manifest, "x64");
        assert_eq!(x64.len(), 1);
        assert_eq!(x64[0].file_name, "WhatsApp.Root_2.2639.100.0_x64.msix");
        assert_eq!(
            select_arch_application_packages(&manifest, "arm64")[0].file_name,
            "WhatsApp.Root_2.2639.100.0_arm64.msix"
        );
        assert!(select_arch_application_packages(&manifest, "x86").is_empty());
        assert!(select_arch_application_packages(&manifest, "neutral").is_empty());
    }

    #[test]
    fn package_manifest_reads_dependencies_and_application() {
        let manifest = parse_package_manifest(PACKAGE_XML).unwrap();
        assert_eq!(manifest.identity.name, "5319275A.WhatsAppDesktop");
        assert_eq!(manifest.identity.version, "2.2587.9.0");
        assert_eq!(
            manifest.identity.processor_architecture.as_deref(),
            Some("x64")
        );
        assert_eq!(manifest.target_device_families.len(), 2);
        assert_eq!(manifest.target_device_families[0].name, "Windows.Universal");
        assert_eq!(
            manifest.target_device_families[0].min_version,
            "10.0.17763.0"
        );
        assert_eq!(
            manifest.target_device_families[1].max_version_tested,
            "10.0.19041.0"
        );
        assert_eq!(manifest.package_dependencies.len(), 3);
        assert_eq!(
            manifest.package_dependencies[0].name,
            "Microsoft.WindowsAppRuntime.1.6"
        );
        assert_eq!(
            manifest.package_dependencies[0].min_version,
            "6000.318.2304.0"
        );
        assert_eq!(
            manifest.package_dependencies[0].publisher.as_deref(),
            Some("CN=Microsoft Corporation")
        );
        assert_eq!(manifest.package_dependencies[1].publisher, None);
        assert_eq!(manifest.applications.len(), 1);
        assert_eq!(manifest.applications[0].id, "WhatsApp.Root");
        assert_eq!(manifest.applications[0].executable, "WhatsApp.Root.exe");
        assert_eq!(
            manifest.applications[0].entry_point.as_deref(),
            Some("Windows.FullTrustApplication")
        );
    }

    /// Live activatable-class declarations (from the extracted WhatsApp
    /// main package manifest, 2026-10-12), trimmed to two servers. The
    /// live `Extension` redeclares the default namespace; class entries
    /// are empty elements.
    const ACTIVATABLE_XML: &str = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Applications>
    <Application Id="WhatsApp.Root" Executable="WhatsApp.Root.exe" EntryPoint="Windows.FullTrustApplication"/>
  </Applications>
  <Extensions>
    <Extension Category="windows.activatableClass.inProcessServer" xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
      <InProcessServer>
        <Path>Microsoft.Windows.ApplicationModel.Resources.dll</Path>
        <ActivatableClass ActivatableClassId="Microsoft.Windows.ApplicationModel.Resources.KnownResourceQualifierName" ThreadingModel="both" />
        <ActivatableClass ActivatableClassId="Microsoft.Windows.ApplicationModel.Resources.ResourceCandidate" ThreadingModel="both" />
        <ActivatableClass ActivatableClassId="Microsoft.Windows.ApplicationModel.Resources.ResourceLoader" ThreadingModel="both" />
        <ActivatableClass ActivatableClassId="Microsoft.Windows.ApplicationModel.Resources.ResourceManager" ThreadingModel="both" />
        <ActivatableClass ActivatableClassId="Microsoft.Windows.Globalization.ApplicationLanguages" ThreadingModel="both" />
      </InProcessServer>
    </Extension>
    <Extension xmlns:uap8="http://schemas.microsoft.com/appx/manifest/uap/windows10/8" Category="windows.activatableClass.inProcessServer">
      <uap8:InProcessServer>
        <uap8:Path>Microsoft.UI.Xaml.dll</uap8:Path>
        <uap8:ActivatableClass ActivatableClassId="Microsoft.UI.Xaml.Application" ThreadingModel="both" />
      </uap8:InProcessServer>
    </Extension>
  </Extensions>
</Package>"#;

    #[test]
    fn activatable_classes_follow_their_in_process_server() {
        let classes = parse_activatable_classes(ACTIVATABLE_XML).unwrap();
        // 5 entries from the first server + 1 from the prefixed one.
        assert_eq!(classes.len(), 6);
        assert_eq!(
            classes[0],
            ActivatableClass {
                class_id: "Microsoft.Windows.ApplicationModel.Resources.KnownResourceQualifierName"
                    .to_owned(),
                server_path: "Microsoft.Windows.ApplicationModel.Resources.dll".to_owned(),
            }
        );
        assert_eq!(
            classes[4].class_id,
            "Microsoft.Windows.Globalization.ApplicationLanguages"
        );
        assert_eq!(
            classes[5],
            ActivatableClass {
                class_id: "Microsoft.UI.Xaml.Application".to_owned(),
                server_path: "Microsoft.UI.Xaml.dll".to_owned(),
            }
        );
    }

    #[test]
    fn activatable_classes_without_a_path_or_id_are_skipped() {
        let classes = parse_activatable_classes(
            r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
               <Extensions>
                 <Extension Category="windows.activatableClass.inProcessServer">
                   <InProcessServer>
                     <ActivatableClass ActivatableClassId="No.Server.Class" />
                     <Path>Later.Server.dll</Path>
                     <ActivatableClass ActivatableClassId="Later.Server.Class" />
                     <ActivatableClass ThreadingModel="both" />
                     <InProcessServer>
                        <Path>Inner.dll</Path>
                     </InProcessServer>
                   </InProcessServer>
                 </Extension>
               </Extensions>
             </Package>"#,
        )
        .unwrap();
        // The orphan (Path after the entry) keeps the empty server path,
        // the id-less entry is dropped, the empty inner server adds none.
        assert_eq!(
            classes,
            vec![
                ActivatableClass {
                    class_id: "No.Server.Class".to_owned(),
                    server_path: String::new(),
                },
                ActivatableClass {
                    class_id: "Later.Server.Class".to_owned(),
                    server_path: "Later.Server.dll".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn missing_required_attribute_is_reported() {
        let error =
            parse_package_manifest("<Package><Identity Name=\"A\"/></Package>").unwrap_err();
        assert!(error.to_string().contains("Publisher"), "got: {error}");
    }

    #[test]
    fn bad_number_attribute_is_reported() {
        let error = parse_bundle_manifest(
            r#"<Bundle xmlns="http://schemas.microsoft.com/appx/2013/bundle"><Packages>
               <Package Type="application" Version="1" Architecture="x64" FileName="a.msix" Offset="xx" Size="1"/>
               </Packages></Bundle>"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Offset"), "got: {error}");
    }

    #[test]
    fn windows_calculator_manifest_parses_cleanly() {
        let xml = r##"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="Microsoft.WindowsCalculator"
            Publisher="CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US"
            Version="11.2607.0.0"
            ProcessorArchitecture="x64"/>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Universal" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0"/>
    <PackageDependency Name="Microsoft.UI.Xaml.2.8" MinVersion="8.2501.31001.0" Publisher="CN=Microsoft Corporation"/>
    <PackageDependency Name="Microsoft.NET.Native.Runtime.2.2" MinVersion="2.2.28604.0" Publisher="CN=Microsoft Corporation"/>
  </Dependencies>
  <Applications>
    <Application Id="App" Executable="CalculatorApp.exe" EntryPoint="Calculator.App">
      <uap:VisualElements DisplayName="Calculator" Description="Windows Calculator" BackgroundColor="#0078D4"/>
    </Application>
  </Applications>
</Package>"##;
        let manifest = parse_package_manifest(xml).expect("Calculator manifest must parse cleanly");
        assert_eq!(manifest.identity.name, "Microsoft.WindowsCalculator");
        assert_eq!(
            manifest.identity.processor_architecture.as_deref(),
            Some("x64")
        );
        assert_eq!(manifest.applications.len(), 1);
        assert_eq!(manifest.applications[0].id, "App");
        assert_eq!(manifest.applications[0].executable, "CalculatorApp.exe");
        assert_eq!(
            manifest.applications[0].entry_point.as_deref(),
            Some("Calculator.App")
        );
        assert!(
            manifest
                .package_dependencies
                .iter()
                .any(|d| d.name == "Microsoft.UI.Xaml.2.8")
        );
        assert!(
            manifest
                .package_dependencies
                .iter()
                .any(|d| d.name == "Microsoft.NET.Native.Runtime.2.2")
        );
    }

    #[test]
    fn windows_calculator_activatable_classes_parsed() {
        let xml = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Extensions>
    <Extension Category="windows.activatableClass.inProcessServer">
      <InProcessServer>
        <Path>CalculatorApp.dll</Path>
        <ActivatableClass ActivatableClassId="CalculatorApp.ViewModel.Common.Utilities" ThreadingModel="both"/>
        <ActivatableClass ActivatableClassId="CalculatorApp.ViewModel.Common.NavCategory" ThreadingModel="both"/>
        <ActivatableClass ActivatableClassId="CalculatorApp.ViewModel.Common.DateCalculation.DateCalculationEngine" ThreadingModel="both"/>
      </InProcessServer>
    </Extension>
  </Extensions>
</Package>"#;
        let classes =
            parse_activatable_classes(xml).expect("Calculator activatable classes must parse");
        assert_eq!(classes.len(), 3);
        assert_eq!(
            classes[0].class_id,
            "CalculatorApp.ViewModel.Common.Utilities"
        );
        assert_eq!(classes[0].server_path, "CalculatorApp.dll");
        assert_eq!(
            classes[1].class_id,
            "CalculatorApp.ViewModel.Common.NavCategory"
        );
        assert_eq!(
            classes[2].class_id,
            "CalculatorApp.ViewModel.Common.DateCalculation.DateCalculationEngine"
        );
    }
}
