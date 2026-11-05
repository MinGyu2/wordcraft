//! Footnote/endnote separators: the model's `None` is the default separator. The writer emits a
//! fixed XML for it, and the reader must read exactly that XML back as `None` (a stable round
//! trip); any other separator is the document's own and is kept.

use std::io::{Cursor, Read, Write};

use wordcraft_doc::para::NoteKind;
use wordcraft_doc::props::CharProps;
use wordcraft_doc::roundtrip::roundtrip_equivalent;
use wordcraft_doc::{Document, InlineObject, Paragraph, PartKind, para_block};

/// A document with one footnote and one endnote referenced from the body.
fn noted() -> Document {
    let mut d = Document::new();
    let foot = d.add_part(PartKind::Footnote, vec![para_block(Paragraph::with_text("foot text", CharProps::default()))]);
    let end = d.add_part(PartKind::Endnote, vec![para_block(Paragraph::with_text("end text", CharProps::default()))]);
    let mut p = Paragraph::with_text("Body", CharProps::default());
    p.insert_object(4, InlineObject::NoteRef { kind: NoteKind::Footnote, id: foot, custom: String::new() }, &CharProps::default()).unwrap();
    let end_at = p.text.len();
    p.insert_object(end_at, InlineObject::NoteRef { kind: NoteKind::Endnote, id: end, custom: String::new() }, &CharProps::default()).unwrap();
    d.body = vec![para_block(p)];
    d
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn with_part(bytes: &[u8], name: &str, data: &str) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let n = f.name().to_string();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.start_file(n.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(if n == name { data.as_bytes() } else { &b }).unwrap();
    }
    out.finish().unwrap().into_inner()
}

const DEFAULT_SEP: &str = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote>"#;

#[test]
fn default_separators_round_trip_as_none() {
    let d = noted();
    let bytes = wordcraft_docx::write(&d).unwrap();
    let xml = part(&bytes, "word/footnotes.xml");
    assert!(xml.contains(DEFAULT_SEP), "writer default changed: {xml}");
    let r = wordcraft_docx::read(&bytes).unwrap();
    assert!(r.footnote_separators.is_empty() && r.endnote_separators.is_empty());
    // A loaded document (what an editor holds) writes and reads back equivalent: before the
    // reader recognised the default it came back as an explicit `Some` separator.
    let again = wordcraft_docx::write(&r).unwrap();
    assert_eq!(part(&again, "word/footnotes.xml"), xml, "same default XML again");
    let r2 = wordcraft_docx::read(&again).unwrap();
    assert!(r2.footnote_separators.is_empty() && r2.endnote_separators.is_empty());
    assert!(roundtrip_equivalent(&r, &r2).unwrap());
}

#[test]
fn own_separator_is_kept_and_compared() {
    let mut d = noted();
    d.footnote_separators.separator = Some(vec![para_block(Paragraph::with_text("* * *", CharProps::default()))]);
    let r = wordcraft_docx::read(&wordcraft_docx::write(&d).unwrap()).unwrap();
    let own = r.footnote_separators.separator.as_ref().expect("document's own separator kept");
    assert!(own.iter().filter_map(|b| b.as_para()).any(|p| p.plain_text().contains("* * *")));
    assert!(r.footnote_separators.continuation_separator.is_none(), "untouched kind stays default");
    // A model that lost its own separator is not equivalent to one that has it.
    let mut lost = r.clone();
    lost.footnote_separators.separator = None;
    assert!(!roundtrip_equivalent(&r, &lost).unwrap());
}

#[test]
fn near_default_separators_stay_the_documents_own() {
    let bytes = wordcraft_docx::write(&noted()).unwrap();
    let xml = part(&bytes, "word/footnotes.xml");
    for (name, variant) in [
        ("paragraph attribute", DEFAULT_SEP.replacen("<w:p>", r#"<w:p w:rsidR="00A1B2C3">"#, 1)),
        ("other spacing", DEFAULT_SEP.replacen(r#"w:after="0""#, r#"w:after="120""#, 1)),
        ("extra run", DEFAULT_SEP.replacen("</w:r></w:p>", "</w:r><w:r><w:t>x</w:t></w:r></w:p>", 1)),
        ("run formatting", DEFAULT_SEP.replacen("<w:r><w:separator/>", r#"<w:r><w:rPr><w:color w:val="FF0000"/></w:rPr><w:separator/>"#, 1)),
    ] {
        let patched = with_part(&bytes, "word/footnotes.xml", &xml.replacen(DEFAULT_SEP, &variant, 1));
        let r = wordcraft_docx::read(&patched).unwrap();
        assert!(r.footnote_separators.separator.is_some(), "{name}: must not be normalized away");
    }
}

#[test]
fn first_separator_of_a_kind_counts_even_when_default() {
    let bytes = wordcraft_docx::write(&noted()).unwrap();
    let xml = part(&bytes, "word/footnotes.xml");
    let second = r#"<w:footnote w:type="separator" w:id="-2"><w:p><w:r><w:t>later</w:t></w:r></w:p></w:footnote>"#;
    let patched = with_part(&bytes, "word/footnotes.xml", &xml.replacen(DEFAULT_SEP, &format!("{DEFAULT_SEP}{second}"), 1));
    let r = wordcraft_docx::read(&patched).unwrap();
    assert!(r.footnote_separators.separator.is_none(), "the first (default) separator decides");
}
