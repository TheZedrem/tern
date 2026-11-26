//! A panel's icon as its manifest gives it (spec 0018, Icons): the name
//! of one of Tern's own, or an SVG file from the plugin's folder.
//!
//! The file is untrusted input to the SVG renderer (TM-plug-24), and that
//! renderer would follow an `<image>` to a file, load the system fonts for
//! a `<text>` and expand nested `<use>`. So Tern only accepts a file made
//! of plain shapes, which can reference nothing, and checks that itself
//! before the renderer ever sees the bytes.

use super::manifest::{Manifest, shown};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum PanelIcon {
    /// As written in the manifest; the sidebar looks it up in Tern's own
    /// icons and draws the default for a name that isn't there.
    Named(String),
    /// A checked SVG from the plugin's folder.
    Image(Arc<[u8]>),
}

const MAX_BYTES: u64 = 16 * 1024;

/// Everything an icon may be made of. None of these can point at a file,
/// a URL, a font or another element.
const ELEMENTS: [&str; 10] = [
    "svg", "g", "path", "circle", "ellipse", "rect", "line", "polyline", "polygon", "title",
];

/// Doctypes, comments, CDATA, processing instructions and entities: ways
/// to hide an element from the check below, or to grow on parsing.
const MARKUP: [&str; 3] = ["<!", "<?", "&"];

/// `icon` if it names a file: a kebab-case name ending in `.svg`, so
/// never a path.
fn file_name(icon: &str) -> Option<&str> {
    let stem = icon.strip_suffix(".svg")?;
    let plain = stem
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (!stem.is_empty() && plain).then_some(icon)
}

/// `manifest` with every panel icon that names a file replaced by that
/// file's bytes, or why one of them can't be used. Reads files: call it
/// off the UI thread.
pub(super) fn load(folder: &Path, mut manifest: Manifest) -> Result<Manifest, String> {
    for panel in &mut manifest.panels {
        let Some(PanelIcon::Named(name)) = &panel.icon else {
            continue;
        };
        let Some(file) = file_name(name) else {
            continue;
        };
        let image = read(&folder.join(file))
            .and_then(|text| check(&text).map(|()| text))
            .and_then(|text| drawable(&text).map(|()| text))
            .map_err(|why| format!("its icon `{file}` {why}"))?;
        panel.icon = Some(PanelIcon::Image(image.into_bytes().into()));
    }
    Ok(manifest)
}

fn read(path: &Path) -> Result<String, String> {
    let unreadable = |error: std::io::Error| format!("can't be read: {error}");
    // A link could name any file on the machine.
    if !path.symlink_metadata().map_err(unreadable)?.is_file() {
        return Err("isn't a file in the plugin's folder".into());
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(unreadable)?;
    if text.len() as u64 > MAX_BYTES {
        return Err("is larger than 16 KiB".into());
    }
    Ok(text)
}

/// Whether the renderer can read `text`, so a file with a typo is the
/// author's error here and not an empty button in the strip. Only for
/// text that passed `check`: the renderer is what `check` protects.
fn drawable(text: &str) -> Result<(), String> {
    gpui::SvgRenderer::new(Arc::new(()))
        .parse_svg(text.as_bytes())
        .map(|_| ())
        .map_err(|error| format!("isn't an SVG that can be drawn: {error}"))
}

/// Whether `text` is an SVG of plain shapes and nothing else.
fn check(text: &str) -> Result<(), String> {
    if !text.trim_start().starts_with("<svg") {
        return Err("doesn't start with `<svg`".into());
    }
    if MARKUP.iter().any(|markup| text.contains(markup)) {
        return Err("holds a comment, a doctype, an entity or the like; \
             only plain shapes are drawn"
            .into());
    }
    for tag in text.split('<').skip(1) {
        let name = tag
            .strip_prefix('/')
            .unwrap_or(tag)
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or_default();
        if !ELEMENTS.contains(&name) {
            return Err(format!(
                "holds a `<{}>`; only plain shapes are drawn ({})",
                shown(name, 32),
                ELEMENTS.join(", ")
            ));
        }
    }
    Ok(())
}

/// Spec 0018, Design, "A plugin's own icon" and threat model 0005,
/// TM-plug-24: which files are accepted as an icon, and that anything
/// else makes the plugin unusable with a reason naming the file.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use crate::plugins::manifest::parse;
    use std::fs;

    /// What discovery makes of a panel with `icon: whale.svg` in `folder`.
    fn loaded(folder: &Path) -> Result<Option<PanelIcon>, String> {
        let text = "api: 1\nid: whale\nname: Whale\nrun: [./x]\n\
                    panels: [{ id: p, title: P, icon: whale.svg }]\n";
        let manifest = parse("whale", text).expect("the manifest is usable");
        load(folder, manifest).map(|manifest| manifest.panels[0].icon.clone())
    }

    /// The same, with `bytes` as the folder's `whale.svg`.
    fn icon_of(folder: &TempDir, bytes: &[u8]) -> Result<Option<PanelIcon>, String> {
        fs::write(folder.path().join("whale.svg"), bytes).expect("write whale.svg");
        loaded(folder.path())
    }

    fn accepted(folder: &TempDir, svg: &str) {
        assert_eq!(
            icon_of(folder, svg.as_bytes()),
            Ok(Some(PanelIcon::Image(svg.as_bytes().into()))),
            "{svg:?} is not accepted as it is"
        );
    }

    fn rejected(folder: &TempDir, bytes: &[u8]) {
        let shown = String::from_utf8_lossy(bytes);
        let shown: String = shown.chars().take(120).collect();
        match icon_of(folder, bytes) {
            Ok(icon) => panic!("{shown:?} is accepted: {icon:?}"),
            Err(reason) => assert!(
                reason.contains("whale.svg"),
                "the reason for {shown:?} doesn't name the file: {reason:?}"
            ),
        }
    }

    fn utf16(text: &str, little_endian: bool, bom: bool) -> Vec<u8> {
        let units = bom.then_some(0xFEFF).into_iter().chain(text.encode_utf16());
        units
            .flat_map(|unit| {
                if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect()
    }

    /// "only the elements `svg`, `g`, `path`, `circle`, `ellipse`, `rect`,
    /// `line`, `polyline`, `polygon` and `title`": each one passes, empty
    /// or with an end tag, and the file's bytes are what is drawn.
    #[test]
    fn each_plain_shape_is_accepted() {
        let folder = TempDir::new("icon-allowed");
        let elements = [
            "svg", "g", "path", "circle", "ellipse", "rect", "line", "polyline", "polygon", "title",
        ];
        for element in elements {
            accepted(&folder, &format!("<svg><{element}/></svg>"));
            accepted(&folder, &format!("<svg><{element}></{element}></svg>"));
            accepted(
                &folder,
                &format!("<svg><g><{element} id=\"a\" fill=\"none\" /></g></svg>"),
            );
        }
    }

    /// A shapes-only file as an author writes it: attributes, line breaks
    /// and tabs between them, text in a title, leading whitespace.
    #[test]
    fn a_shapes_only_file_is_accepted_as_written() {
        let folder = TempDir::new("icon-realistic");
        accepted(
            &folder,
            "\n\t  <svg xmlns=\"http://www.w3.org/2000/svg\"\n     viewBox=\"0 0 24 24\" \
             fill=\"none\"\tstroke=\"currentColor\">\r\n  <title>A whale > a fish</title>\n  \
             <g stroke-width=\"2\">\n    <path\n      d=\"M2 14h20c0 4-3 6-8 6H9c-4 0-7-2-7-6z\"/>\n    \
             <circle cx=\"12\" cy=\"12\" r=\"3\"></circle>\n    \
             <ellipse cx=\"1\" cy=\"1\" rx=\"1\" ry=\"2\"/>\n    \
             <rect x=\"3\" y=\"9\" width=\"4\" height=\"4\"/>\n    \
             <line x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"/>\n    \
             <polyline points=\"0,0 1,1\"/>\n    <polygon points=\"0,0 1,1 2,0\"/>\n  \
             </g>\n</svg>\n",
        );
    }

    /// The conditions name no `viewBox`, size or namespace: a well-formed
    /// shapes-only file without them passes, down to an empty root.
    #[test]
    fn a_well_formed_file_needs_no_view_box_or_size() {
        let folder = TempDir::new("icon-no-size");
        let bare = [
            "<svg/>",
            "<svg></svg>",
            "<svg />",
            "<svg\n></svg\n>",
            "<svg><path d=\"M0 0h4v4z\"/></svg>",
            "<svg><g><rect x=\"1\" y=\"1\" width=\"2\" height=\"2\"/></g></svg>\n",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle cx=\"2\" cy=\"2\" r=\"1\"/></svg>",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            "<svg fill='none'><path d='M0 0h4'/></svg>",
            "<svg width=\"24\" height=\"24\"><polygon points=\"0,0 1,1 2,0\"/></svg>",
            "<svg viewBox=\"0 0 24 24\"><line x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"/></svg>",
            "<svg><title>Only a title</title></svg>",
        ];
        for svg in bare {
            accepted(&folder, svg);
        }
    }

    /// "the renderer can read it: it is well-formed XML": a file made of
    /// the allowed names only, which is no XML document, is the author's
    /// error and not an empty button in the strip.
    #[test]
    fn a_file_that_is_not_well_formed_is_rejected() {
        let folder = TempDir::new("icon-malformed");
        let malformed = [
            // Cut off.
            "<svg",
            "<svg ",
            "<svg>",
            "<svg><path",
            "<svg><path d=\"M0 0\"",
            "<svg><path d=\"M0 0",
            "<svg><path d=\"M0 0\"/>",
            "<svg><g><path/></g>",
            "<svg></svg",
            "<svg></",
            // Unclosed and mismatched tags.
            "<svg><g></svg>",
            "<svg><g><path/></svg>",
            "<svg><path></svg>",
            "<svg><g></path></svg>",
            "<svg><g><rect></g></rect></svg>",
            "<svg></g>",
            "<svg></svg></svg>",
            "<svg><path/></g></svg>",
            "<svg><title>x</svg>",
            // A second name after `/`, which a text check reads as `svg`
            // or `g`.
            "<svg/image href=\"x\"/>",
            "<svg><g/image/></svg>",
            "<svg><path/d=\"M0 0\"/></svg>",
            // Attributes.
            "<svg><path d=\"M0 0\" d=\"M1 1\"/></svg>",
            "<svg viewBox=\"0 0 1 1\" viewBox=\"0 0 2 2\"/>",
            "<svg><path d=M0/></svg>",
            "<svg width=24 height=24></svg>",
            "<svg><path d/></svg>",
            "<svg><path d=\"M0 0\"fill=\"none\"/></svg>",
            "<svg><path d=\"M0 0'/></svg>",
            "<svg><path =\"M0 0\"/></svg>",
            // Two roots, and text around the root.
            "<svg/><svg/>",
            "<svg></svg><svg></svg>",
            "<svg><path/></svg><g/>",
            "<svg><path/></svg><path d=\"M0 0\"/>",
            "<svg/>whale",
            "<svg><path/></svg> trailing text",
            "<svg><path/></svg>>",
            // End tags that carry more than a name.
            "<svg><g></g fill=\"none\"></svg>",
            "<svg></svg/>",
        ];
        for svg in malformed {
            rejected(&folder, svg.as_bytes());
        }
    }

    /// "So no `use`, `image`, `text`, `style`, `filter`, script or link",
    /// nor any other element, wherever it stands and however it is written.
    #[test]
    fn any_other_element_is_rejected() {
        let folder = TempDir::new("icon-elements");
        let elements = [
            "use",
            "image",
            "text",
            "style",
            "filter",
            "script",
            "a",
            "foreignObject",
            "svg:image",
            "IMAGE",
            "Image",
            "USE",
            "Svg",
            "PATH",
            "svg:use",
            "svg:path",
            "xi:include",
            "tspan",
            "textPath",
            "defs",
            "symbol",
            "marker",
            "mask",
            "clipPath",
            "pattern",
            "linearGradient",
            "feImage",
            "animate",
            "set",
            "switch",
            "metadata",
            "desc",
            "iframe",
            "link",
            "paths",
            "gg",
            "title2",
        ];
        for element in elements {
            for svg in [
                format!("<svg><{element}/></svg>"),
                format!("<svg><{element} href=\"file:///etc/passwd\"></{element}></svg>"),
                format!("<svg><g><g><{element} xlink:href=\"#a\"/></g></g></svg>"),
                format!("<svg><title><{element}/></title></svg>"),
                format!("<svg><path d=\"M0 0\"/><{element}\thref=\"x\"/></svg>"),
                format!("<svg><{element}\n  href=\"x\"\n/></svg>"),
                format!("<svg><{element}\r\nhref=\"x\"/></svg>"),
                format!("<svg></svg><{element}/>"),
                format!("<svg><path/></{element}></svg>"),
                format!("<svg><{element}"),
                format!("<svg><{element}>"),
                format!("<svg><{element}/"),
            ] {
                rejected(&folder, svg.as_bytes());
            }
        }
    }

    /// Ways to get an element past a check that only reads text.
    #[test]
    fn an_element_can_not_be_hidden_from_the_check() {
        let folder = TempDir::new("icon-hostile");
        let hidden = [
            // The root's name only starts like `svg`.
            "<svgimage href=\"x\"/>",
            "<svg-image/>",
            "<svgs><image/></svgs>",
            "<svg:svg><svg:image/></svg:svg>",
            // Space, control characters or nothing before the name.
            "<svg>< image href=\"x\"/></svg>",
            "<svg><\timage/></svg>",
            "<svg><\0image/></svg>",
            "<svg><image\0/></svg>",
            "<svg><></svg>",
            "<svg><</svg>",
            "<svg></ image></svg>",
            "<svg><//image></svg>",
            // Inside an attribute's value.
            "<svg><path d=\"<image href='x'/>\"/></svg>",
            "<svg><path d='\"><image href=\"x\"/>'/></svg>",
            // After whitespace that isn't XML's.
            "<svg><image\u{a0}href=\"x\"/></svg>",
            "<svg><image\u{2003}href=\"x\"/></svg>",
            "<svg><use\u{3000}href=\"#a\"/></svg>",
            // A second root, and an unclosed one.
            "<svg><path/></svg><use href=\"#a\"/>",
            "<svg><g><g><g><image",
            // Other casings of the root.
            "<SVG><path/></SVG>",
            "<svg><path/></SVG>",
        ];
        for svg in hidden {
            rejected(&folder, svg.as_bytes());
        }
    }

    /// "no `<!`, `<?` or `&` anywhere: no doctype, entity, comment or
    /// processing instruction."
    #[test]
    fn doctypes_comments_cdata_instructions_and_entities_are_rejected() {
        let folder = TempDir::new("icon-markup");
        let markup = [
            "<!DOCTYPE svg><svg><path/></svg>",
            "<svg><!DOCTYPE svg></svg>",
            "<svg><!doctype svg></svg>",
            "<!DOCTYPE svg [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]>\
             <svg><title>&b;</title></svg>",
            "<svg><!-- a comment --></svg>",
            "<svg><!----></svg>",
            "<svg><!-- <image href=\"x\"/> --></svg>",
            "<svg><path/></svg><!-- after -->",
            "<svg><![CDATA[<image href=\"x\"/>]]></svg>",
            "<svg><title><![CDATA[text]]></title></svg>",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><svg><path/></svg>",
            "<?xml version=\"1.0\"?>\n<svg/>",
            "<svg><?xml-stylesheet href=\"x.css\"?></svg>",
            "<svg><?php ?></svg>",
            "<svg><path/></svg><?end?>",
            "<svg><!></svg>",
            "<svg><?></svg>",
            "<svg><title>Tom &amp; Jerry</title></svg>",
            "<svg><title>&#60;image/&#62;</title></svg>",
            "<svg><title>&#x3c;image/></title></svg>",
            "<svg><title>&lt;</title></svg>",
            "<svg><title>&undefined;</title></svg>",
            "<svg><path d=\"M0 0\" id=\"a&amp;b\"/></svg>",
            "<svg><title>R&D</title></svg>",
            "<svg><path/></svg>&",
            "<svg &></svg>",
        ];
        for svg in markup {
            rejected(&folder, svg.as_bytes());
        }
    }

    /// "UTF-8 that starts with `<svg`": whitespace may come first,
    /// nothing else may.
    #[test]
    fn a_file_that_does_not_start_with_svg_is_rejected() {
        let folder = TempDir::new("icon-start");
        let not_svg = [
            "",
            " ",
            "\n\n\t",
            "whale",
            "<",
            "<sv",
            "svg",
            "x<svg><path/></svg>",
            "icon: <svg><path/></svg>",
            "<path d=\"M0 0\"/>",
            "<g><svg><path/></svg></g>",
            "<title>x</title><svg/>",
            "<html><body><svg><path/></svg></body></html>",
            "</svg><svg/>",
            "\u{feff}<svg><path/></svg>",
            "\0<svg><path/></svg>",
            "{\"icon\": \"<svg/>\"}",
        ];
        for text in not_svg {
            rejected(&folder, text.as_bytes());
        }
        accepted(&folder, " \r\n\t<svg><path/></svg>");
    }

    /// "UTF-8": bytes of another encoding, a compressed SVG or an image
    /// of another format are no icon.
    #[test]
    fn bytes_that_are_not_utf8_are_rejected() {
        let folder = TempDir::new("icon-bytes");
        let svg = "<svg><path d=\"M0 0\"/></svg>";
        rejected(&folder, b"<svg><title>\xff</title></svg>");
        rejected(&folder, b"<svg><path d=\"M0 0\"/></svg>\x80");
        rejected(&folder, b"\xff<svg><path/></svg>");
        // Latin-1 `é`.
        rejected(&folder, b"<svg><title>caf\xe9</title></svg>");
        // A lone surrogate and an overlong `<`.
        rejected(&folder, b"<svg><title>\xed\xa0\x80</title></svg>");
        rejected(&folder, b"<svg>\xc0\xbcimage/></svg>");
        // The header of a gzip stream (`.svgz`), then of a PNG.
        let gzip = [
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xb3, 0x29, 0x2e, 0x4b,
            0xd7, 0xb7, 0x03, 0x00, 0x8d, 0x4d, 0x4a, 0x1e, 0x06, 0x00, 0x00, 0x00,
        ];
        rejected(&folder, &gzip);
        rejected(&folder, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        for little_endian in [true, false] {
            for bom in [true, false] {
                rejected(&folder, &utf16(svg, little_endian, bom));
            }
        }
        accepted(&folder, "<svg><title>Wal \u{1f40b} café</title></svg>");
    }

    /// "at most 16 KiB": exactly 16 KiB is not over, one byte more is,
    /// counted in bytes and whatever the file holds.
    #[test]
    fn a_file_over_16_kib_is_rejected() {
        const LIMIT: usize = 16 * 1024;
        let folder = TempDir::new("icon-size");
        let padded = |bytes: usize, fill: &str| {
            let (open, close) = ("<svg><title>", "</title></svg>");
            let room = bytes - open.len() - close.len();
            assert_eq!(room % fill.len(), 0, "the fill doesn't fit");
            let svg = format!("{open}{}{close}", fill.repeat(room / fill.len()));
            assert_eq!(svg.len(), bytes);
            svg
        };
        accepted(&folder, &padded(LIMIT, "x"));
        accepted(&folder, &padded(LIMIT - 1, "x"));
        rejected(&folder, padded(LIMIT + 1, "x").as_bytes());
        rejected(&folder, padded(LIMIT * 2, "x").as_bytes());
        // 16 KiB of bytes, not of characters: `é` is two bytes.
        let two_bytes = padded(LIMIT + 2, "\u{e9}");
        assert!(two_bytes.chars().count() < LIMIT);
        rejected(&folder, two_bytes.as_bytes());
        // Whitespace counts too, before the root and after it.
        let leading = format!("{}<svg/>", " ".repeat(LIMIT - 5));
        assert_eq!(leading.len(), LIMIT + 1);
        rejected(&folder, leading.as_bytes());
        let trailing = format!("<svg/>{}", "\n".repeat(LIMIT - 5));
        rejected(&folder, trailing.as_bytes());
        // Far over, and nothing an SVG is made of: still only a reason.
        rejected(&folder, &vec![b'<'; 4 * 1024 * 1024]);
        rejected(&folder, &vec![0xff; 4 * 1024 * 1024]);
    }

    /// AC9: "a missing one lists the plugin as unusable with the reason".
    #[test]
    fn a_missing_file_is_rejected() {
        let folder = TempDir::new("icon-missing");
        let reason = loaded(folder.path()).expect_err("no whale.svg, yet usable");
        assert!(reason.contains("whale.svg"), "{reason:?}");
        // Nor is it looked for in a folder that isn't there.
        let reason = loaded(&folder.path().join("gone")).expect_err("no folder, yet usable");
        assert!(reason.contains("whale.svg"), "{reason:?}");
    }

    /// "names a file directly in the plugin's folder": a folder of that
    /// name is no file, whatever it holds.
    #[test]
    fn a_folder_named_like_the_icon_is_rejected() {
        let folder = TempDir::new("icon-folder");
        let inner = folder.path().join("whale.svg");
        fs::create_dir_all(&inner).expect("create a folder");
        fs::write(inner.join("whale.svg"), "<svg><path/></svg>").expect("write");
        let reason = loaded(folder.path()).expect_err("a folder is accepted");
        assert!(reason.contains("whale.svg"), "{reason:?}");
    }

    /// "never a path, and never a link" (TM-plug-24: "never a path or a
    /// link"): a link isn't followed, out of the folder or inside it,
    /// even to a file that would pass.
    #[cfg(unix)]
    #[test]
    fn a_link_is_rejected() {
        use std::os::unix::fs::symlink;
        use std::path::PathBuf;
        let root = TempDir::new("icon-link");
        let folder = root.path().join("whale");
        fs::create_dir_all(&folder).expect("create the plugin's folder");
        let link = folder.join("whale.svg");

        let outside = root.path().join("outside.svg");
        fs::write(&outside, "<svg><path/></svg>").expect("write outside.svg");
        let secret = root.path().join("secret");
        fs::write(&secret, "<svg><title>token</title></svg>").expect("write secret");
        let inside = folder.join("fish.svg");
        fs::write(&inside, "<svg><path/></svg>").expect("write fish.svg");
        let targets = [
            inside,
            PathBuf::from("fish.svg"),
            outside,
            secret,
            PathBuf::from("../outside.svg"),
            root.path().to_path_buf(),
            root.path().join("no-such-file"),
            PathBuf::from("/dev/zero"),
        ];
        for target in targets {
            let _ = fs::remove_file(&link);
            symlink(&target, &link).expect("create a link");
            let reason = loaded(&folder).expect_err("a link was followed");
            assert!(reason.contains("whale.svg"), "{target:?}: {reason:?}");
        }
    }

    /// Only a panel's icon that names a file is read: a panel without
    /// one and a panel naming one of Tern's icons stay as they are, and
    /// one bad file among several panels makes the plugin unusable.
    #[test]
    fn every_panel_gets_its_own_icon() {
        let folder = TempDir::new("icon-panels");
        let text = "api: 1\nid: sea\nname: Sea\nrun: [./x]\npanels:\n\
                    \x20 - { id: a, title: A, icon: whale.svg }\n\
                    \x20 - { id: b, title: B, icon: container }\n\
                    \x20 - { id: c, title: C }\n\
                    \x20 - { id: d, title: D, icon: fish-2.svg }\n";
        let manifest = || parse("sea", text).expect("the manifest is usable");
        let whale = "<svg><path d=\"M0 0\"/></svg>";
        let fish = "<svg><circle r=\"1\"/></svg>";
        fs::write(folder.path().join("whale.svg"), whale).expect("write whale.svg");
        fs::write(folder.path().join("fish-2.svg"), fish).expect("write fish-2.svg");

        let loaded = load(folder.path(), manifest()).expect("both files pass");
        let icons: Vec<_> = loaded
            .panels
            .iter()
            .map(|panel| panel.icon.clone())
            .collect();
        assert_eq!(
            icons,
            [
                Some(PanelIcon::Image(whale.as_bytes().into())),
                Some(PanelIcon::Named("container".into())),
                None,
                Some(PanelIcon::Image(fish.as_bytes().into())),
            ]
        );
        // Everything else the manifest says is as it was.
        let mut as_parsed = manifest();
        as_parsed.panels = loaded.panels.clone();
        assert_eq!(loaded, as_parsed);

        let bad = "<svg><use href=\"#a\"/></svg>";
        fs::write(folder.path().join("fish-2.svg"), bad).expect("write fish-2.svg");
        let reason = load(folder.path(), manifest()).expect_err("a `<use>` is accepted");
        assert!(reason.contains("fish-2.svg"), "{reason:?}");
    }
}
