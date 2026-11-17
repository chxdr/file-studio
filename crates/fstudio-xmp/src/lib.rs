//! Reads the culling marks that Import Studio (and Lightroom, Bridge, Capture
//! One, DxO) keep in XMP sidecars: a star rating, a colour label and a pick or
//! reject flag.
//!
//! This crate only ever **reads**. It never writes, creates or touches a
//! sidecar. The format follows Import Studio's own reader
//! (`istudio-metadata/src/xmp.rs`):
//!
//! - `xmp:Rating` is 0–5; `-1` means rejected.
//! - `xmp:Label` is Red, Yellow, Green, Blue or Purple.
//! - `istudio:Flag` is `pick` or `reject`; `istudio:Stars` holds the stars a
//!   shot keeps while it is rejected (its `xmp:Rating` is then `-1`).
//!
//! Each property may be an attribute of `rdf:Description`
//! (`xmp:Rating="3"`) or a child element (`<xmp:Rating>3</xmp:Rating>`).
//! Prefixes are resolved through `xmlns` declarations, so any prefix bound to
//! the right namespace works.
//!
//! A sidecar that isn't well-formed XML, or has no `rdf:Description`, is
//! "unknown" ([`None`]), never an error for the user to see.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// The RDF namespace.
pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// The XMP basic namespace (`xmp:Rating`, `xmp:Label`).
pub const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
/// Import Studio's own namespace (`istudio:Flag`, `istudio:Stars`).
pub const NS_ISTUDIO: &str = "https://importstudio.app/ns/xmp/1.0/";

/// Sidecars larger than this are not read (real ones are a few KB). They
/// count as unknown.
pub const MAX_SIDECAR_BYTES: u64 = 4 * 1024 * 1024;

/// Pick, reject or neither.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Flag {
    /// Not flagged.
    #[default]
    None,
    /// Picked.
    Pick,
    /// Rejected.
    Reject,
}

/// A Lightroom-style colour label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Label {
    /// Red.
    Red,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Purple.
    Purple,
}

impl Label {
    /// All labels, in Lightroom's order (which is also the sort order).
    pub const ALL: [Label; 5] = [
        Label::Red,
        Label::Yellow,
        Label::Green,
        Label::Blue,
        Label::Purple,
    ];

    /// The label's name as written in sidecars ("Red").
    pub fn as_str(self) -> &'static str {
        match self {
            Label::Red => "Red",
            Label::Yellow => "Yellow",
            Label::Green => "Green",
            Label::Blue => "Blue",
            Label::Purple => "Purple",
        }
    }

    /// Parses a label name, ignoring case and surrounding spaces. Anything
    /// else (custom label sets, "Approved") is no label.
    pub fn parse(s: &str) -> Option<Label> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "red" => Label::Red,
            "yellow" => Label::Yellow,
            "green" => Label::Green,
            "blue" => Label::Blue,
            "purple" => Label::Purple,
            _ => return None,
        })
    }
}

/// The culling marks of one shot.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub struct Marks {
    /// 0 for unrated, otherwise 1–5 stars. A rejected shot keeps its stars.
    pub rating: u8,
    /// The colour label, if any.
    pub label: Option<Label>,
    /// Pick, reject or neither.
    pub flag: Flag,
}

impl Marks {
    /// True when the shot is rejected.
    pub fn is_reject(&self) -> bool {
        self.flag == Flag::Reject
    }
    /// True when the shot is picked.
    pub fn is_pick(&self) -> bool {
        self.flag == Flag::Pick
    }
    /// True when nothing is set (no stars, label or flag).
    pub fn is_empty(&self) -> bool {
        *self == Marks::default()
    }
}

/// The sidecar that belongs to an image: `DSC0001.ARW` → `DSC0001.xmp`
/// (Lightroom, Capture One, DxO and Import Studio all use this).
pub fn sidecar_path(image: &Path) -> PathBuf {
    image.with_extension("xmp")
}

/// True when `name` is an XMP sidecar by its extension, in any case
/// (`a.xmp`, `a.XMP`). A bare `.xmp` dot-file is not.
pub fn is_sidecar_name(name: &str) -> bool {
    match name.rfind('.') {
        Some(0) | None => false,
        Some(i) => name[i + 1..].eq_ignore_ascii_case("xmp"),
    }
}

/// What reading a sidecar from disk gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidecarRead {
    /// There is no such file.
    Missing,
    /// The file exists but couldn't be read or parsed, or is too large.
    Unknown,
    /// The marks in the file.
    Marks(Marks),
}

/// Reads and parses a sidecar file. Never writes. Any failure other than
/// "not found" is [`SidecarRead::Unknown`].
///
/// The caller must not call this on an iCloud placeholder (`SF_DATALESS`),
/// since reading it downloads it (F-016).
pub fn read_file(path: &Path) -> SidecarRead {
    use std::io::Read as _;
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SidecarRead::Missing,
        Err(_) => return SidecarRead::Unknown,
    };
    let mut buf = Vec::new();
    // Read one byte past the limit to tell "exactly the limit" from "more".
    match f.take(MAX_SIDECAR_BYTES + 1).read_to_end(&mut buf) {
        Ok(n) if n as u64 <= MAX_SIDECAR_BYTES => {}
        _ => return SidecarRead::Unknown,
    }
    match parse_bytes(&buf) {
        Some(m) => SidecarRead::Marks(m),
        None => SidecarRead::Unknown,
    }
}

/// Parses sidecar bytes (UTF-8, with or without a byte-order mark). `None`
/// when the bytes aren't UTF-8 or the XML is malformed.
pub fn parse_bytes(bytes: &[u8]) -> Option<Marks> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    parse(std::str::from_utf8(bytes).ok()?)
}

/// Parses sidecar text. `None` when the XML is malformed or there is no
/// `rdf:Description`. Absent or unrecognised properties are defaults.
pub fn parse(xml: &str) -> Option<Marks> {
    let mut r = Reader::from_str(xml);
    let mut scopes = Scopes::default();
    let mut raw = Raw::default();
    let mut desc_depth: Vec<usize> = Vec::new();
    let mut depth = 0usize;
    let mut text_for: Option<Prop> = None;
    let mut saw_description = false;
    loop {
        match r.read_event().ok()? {
            Event::Start(e) => {
                scopes.push(&e);
                depth += 1;
                let q = qname(&e);
                if is_description(&scopes, &q) {
                    saw_description = true;
                    read_attrs(&scopes, &e, &mut raw);
                    desc_depth.push(depth);
                    text_for = None;
                } else if desc_depth.last() == Some(&(depth - 1)) {
                    let (u, l) = scopes.resolve(&q);
                    text_for = managed(u, l);
                } else {
                    text_for = None;
                }
            }
            Event::Empty(e) => {
                scopes.push(&e);
                let q = qname(&e);
                if is_description(&scopes, &q) {
                    saw_description = true;
                    read_attrs(&scopes, &e, &mut raw);
                }
                scopes.pop();
            }
            Event::Text(t) => {
                if let Some(p) = text_for {
                    let v = t.xml_content(quick_xml::XmlVersion::Implicit1_0);
                    raw.set(p, &v);
                }
            }
            Event::End(_) => {
                if desc_depth.last() == Some(&depth) {
                    desc_depth.pop();
                }
                text_for = None;
                depth = depth.checked_sub(1)?;
                scopes.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // An unclosed element is not well-formed.
    if depth != 0 || !saw_description {
        return None;
    }
    Some(raw.into_marks())
}

// ───────────────────────── namespaces ─────────────────────────

/// The `xmlns` declarations of each open element, innermost last.
#[derive(Default)]
struct Scopes(Vec<HashMap<String, String>>);

impl Scopes {
    fn push(&mut self, e: &BytesStart<'_>) {
        let mut m = HashMap::new();
        for a in e.attributes().with_checks(false).flatten() {
            let k = a.key.as_ref().to_string();
            let v = a.value.to_string();
            if let Some(p) = k.strip_prefix("xmlns:") {
                m.insert(p.to_string(), v);
            } else if k == "xmlns" {
                m.insert(String::new(), v);
            }
        }
        self.0.push(m);
    }
    fn pop(&mut self) {
        self.0.pop();
    }
    fn uri(&self, prefix: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find_map(|m| m.get(prefix))
            .map(String::as_str)
    }
    /// A qualified name as (namespace URI, local name).
    fn resolve<'a>(&'a self, qname: &'a str) -> (Option<&'a str>, &'a str) {
        match qname.split_once(':') {
            Some((p, l)) => (self.uri(p), l),
            None => (self.uri(""), qname),
        }
    }
}

fn qname(e: &BytesStart<'_>) -> String {
    e.name().as_ref().to_string()
}

fn is_description(s: &Scopes, q: &str) -> bool {
    matches!(s.resolve(q), (Some(NS_RDF), "Description"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prop {
    Rating,
    Label,
    Flag,
    Stars,
}

fn managed(uri: Option<&str>, local: &str) -> Option<Prop> {
    match (uri, local) {
        (Some(NS_XMP), "Rating") => Some(Prop::Rating),
        (Some(NS_XMP), "Label") => Some(Prop::Label),
        (Some(NS_ISTUDIO), "Flag") => Some(Prop::Flag),
        (Some(NS_ISTUDIO), "Stars") => Some(Prop::Stars),
        _ => None,
    }
}

fn read_attrs(s: &Scopes, e: &BytesStart<'_>, raw: &mut Raw) {
    for a in e.attributes().with_checks(false).flatten() {
        let k = a.key.as_ref().to_string();
        // Unprefixed attributes have no namespace in XML.
        let Some((p, l)) = k.split_once(':') else {
            continue;
        };
        if let Some(prop) = managed(s.uri(p), l)
            && let Ok(v) = a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        {
            raw.set(prop, &v);
        }
    }
}

// ───────────────────────── values ─────────────────────────

/// The raw property values, before they are combined.
#[derive(Default)]
struct Raw {
    rating: Option<i32>,
    label: Option<String>,
    flag: Option<String>,
    stars: Option<i32>,
}

impl Raw {
    fn set(&mut self, p: Prop, v: &str) {
        let v = v.trim();
        match p {
            Prop::Rating => self.rating = parse_number(v),
            Prop::Label => self.label = Some(v.to_string()),
            Prop::Flag => self.flag = Some(v.to_ascii_lowercase()),
            Prop::Stars => self.stars = parse_number(v),
        }
    }

    /// Combines the values the way Import Studio does: a `-1` rating is a
    /// reject that keeps `istudio:Stars`; an explicit `istudio:Flag` wins.
    fn into_marks(self) -> Marks {
        let flag = match self.flag.as_deref() {
            Some("pick") => Flag::Pick,
            Some("reject") => Flag::Reject,
            _ if self.rating == Some(-1) => Flag::Reject,
            _ => Flag::None,
        };
        let stars = match self.rating {
            Some(-1) | None => self.stars.unwrap_or(0),
            Some(r) => r,
        };
        Marks {
            rating: stars.clamp(0, 5) as u8,
            label: self.label.as_deref().and_then(Label::parse),
            flag,
        }
    }
}

/// "3", "3.0" and "2.6" are numbers (rounded); anything else is not.
fn parse_number(v: &str) -> Option<i32> {
    let f = v.parse::<f64>().ok()?;
    f.is_finite().then(|| f.round().clamp(-100.0, 100.0) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(rating: u8, label: Option<Label>, flag: Flag) -> Marks {
        Marks {
            rating,
            label,
            flag,
        }
    }

    /// The exact layout Import Studio's `fresh()` writes.
    fn istudio(attrs: &str) -> String {
        format!(
            "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Import Studio\">\n \
<rdf:RDF xmlns:rdf=\"{NS_RDF}\">\n  \
<rdf:Description rdf:about=\"\"\n    xmlns:xmp=\"{NS_XMP}\"\n    xmlns:istudio=\"{NS_ISTUDIO}\"{attrs}/>\n \
</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>\n"
        )
    }

    #[test]
    fn import_studio_attribute_form() {
        let x = istudio("\n    xmp:Rating=\"3\"\n    xmp:Label=\"Red\"\n    istudio:Flag=\"pick\"");
        assert_eq!(parse(&x), Some(m(3, Some(Label::Red), Flag::Pick)));
    }

    #[test]
    fn reject_minus_one_keeps_stars() {
        let x = istudio(
            "\n    xmp:Rating=\"-1\"\n    istudio:Stars=\"4\"\n    istudio:Flag=\"reject\"",
        );
        assert_eq!(parse(&x), Some(m(4, None, Flag::Reject)));
        // A foreign -1 with no flag is a reject with no stars.
        let x = istudio(" xmp:Rating=\"-1\"");
        assert_eq!(parse(&x), Some(m(0, None, Flag::Reject)));
    }

    #[test]
    fn private_only_reject_keeps_rating() {
        let x = istudio(" xmp:Rating=\"2\" istudio:Flag=\"reject\"");
        assert_eq!(parse(&x), Some(m(2, None, Flag::Reject)));
    }

    #[test]
    fn empty_description_is_default_not_unknown() {
        assert_eq!(parse(&istudio("")), Some(Marks::default()));
    }

    #[test]
    fn element_form_lightroom_style() {
        let x = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="{NS_RDF}">
  <rdf:Description rdf:about="" xmlns:xmp="{NS_XMP}" xmlns:is="{NS_ISTUDIO}"
     xmlns:dc="http://purl.org/dc/elements/1.1/">
   <xmp:Rating> 5 </xmp:Rating>
   <xmp:Label>purple</xmp:Label>
   <is:Flag>PICK</is:Flag>
   <dc:subject><rdf:Bag><rdf:li>Rating</rdf:li></rdf:Bag></dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
        );
        assert_eq!(parse(&x), Some(m(5, Some(Label::Purple), Flag::Pick)));
    }

    #[test]
    fn other_prefixes_and_namespaces() {
        // A prefix named "xmp" bound to some other namespace is not XMP basic.
        let x = format!(
            r#"<rdf:RDF xmlns:rdf="{NS_RDF}"><rdf:Description xmlns:xmp="urn:other" xmlns:a="{NS_XMP}" xmp:Rating="1" a:Rating="4"/></rdf:RDF>"#
        );
        assert_eq!(parse(&x), Some(m(4, None, Flag::None)));
    }

    #[test]
    fn several_descriptions_merge() {
        let x = format!(
            r#"<rdf:RDF xmlns:rdf="{NS_RDF}">
<rdf:Description xmlns:xmp="{NS_XMP}" xmp:Rating="2"/>
<rdf:Description xmlns:xmp="{NS_XMP}" xmp:Label="Blue"/></rdf:RDF>"#
        );
        assert_eq!(parse(&x), Some(m(2, Some(Label::Blue), Flag::None)));
    }

    #[test]
    fn odd_values() {
        let x = istudio(" xmp:Rating=\"9\" xmp:Label=\"Approved\" istudio:Flag=\"maybe\"");
        assert_eq!(parse(&x), Some(m(5, None, Flag::None)));
        let x = istudio(" xmp:Rating=\"2.6\"");
        assert_eq!(parse(&x), Some(m(3, None, Flag::None)));
        let x = istudio(" xmp:Rating=\"lots\"");
        assert_eq!(parse(&x), Some(m(0, None, Flag::None)));
        let x = istudio(" xmp:Rating=\"NaN\"");
        assert_eq!(parse(&x), Some(m(0, None, Flag::None)));
    }

    #[test]
    fn malformed_is_unknown() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("<not xml"), None);
        assert_eq!(parse("<x:xmpmeta><rdf:RDF>"), None);
        assert_eq!(parse("<a><b/></a>"), None); // no Description
        assert_eq!(parse("</a>"), None);
        let truncated = istudio(" xmp:Rating=\"3\"");
        assert_eq!(parse(&truncated[..truncated.len() / 2]), None);
        assert_eq!(parse_bytes(b"\xff\xfe<\x00"), None);
    }

    #[test]
    fn bom_bytes() {
        let mut b = b"\xEF\xBB\xBF".to_vec();
        b.extend(istudio(" xmp:Rating=\"1\"").as_bytes());
        assert_eq!(parse_bytes(&b), Some(m(1, None, Flag::None)));
    }

    #[test]
    fn names() {
        assert!(is_sidecar_name("DSC0001.xmp"));
        assert!(is_sidecar_name("DSC0001.XMP"));
        assert!(is_sidecar_name("Caf\u{e9}.Xmp"));
        assert!(!is_sidecar_name(".xmp"));
        assert!(!is_sidecar_name("xmp"));
        assert!(!is_sidecar_name("a.xmpp"));
        assert_eq!(
            sidecar_path(Path::new("/p/DSC0001.ARW")),
            Path::new("/p/DSC0001.xmp")
        );
    }

    #[test]
    fn files_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let ok = d.path().join("A.XMP");
        std::fs::write(&ok, istudio(" xmp:Rating=\"4\"")).unwrap();
        assert_eq!(read_file(&ok), SidecarRead::Marks(m(4, None, Flag::None)));
        let bad = d.path().join("B.xmp");
        std::fs::write(&bad, "<oops").unwrap();
        assert_eq!(read_file(&bad), SidecarRead::Unknown);
        assert_eq!(read_file(&d.path().join("C.xmp")), SidecarRead::Missing);
        // A folder named like a sidecar can't be read as one.
        let dir = d.path().join("D.xmp");
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(read_file(&dir), SidecarRead::Unknown);
        // Too large.
        let big = d.path().join("E.xmp");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_SIDECAR_BYTES + 1).unwrap();
        assert_eq!(read_file(&big), SidecarRead::Unknown);
        // Nothing was written next to them.
        let n = std::fs::read_dir(d.path()).unwrap().count();
        assert_eq!(n, 4);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_is_unknown() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("A.xmp");
        std::fs::write(&p, istudio(" xmp:Rating=\"4\"")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read anything; only check when the file is really unreadable.
        if std::fs::read(&p).is_err() {
            assert_eq!(read_file(&p), SidecarRead::Unknown);
        }
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}
