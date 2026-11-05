//! References › Table of Contents and Update Table: the engine's TOC (a `TOCHeading` paragraph
//! holding an empty `TOC` field, then generated `TOCn` entries) must survive export -> reopen as
//! the same model (semantic round trip), across inserts, updates and edited second saves.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::json;
use wordcraft_doc::roundtrip::{Equivalence, RoundTripLimits, compare_roundtrip};
use wordcraft_doc::{Block, Document, InlineObject, Paragraph, Path, Pos, StoryRef, para_block};
use wordcraft_engine::{Session, io};

fn headed(heads: &[(&str, u8)]) -> Document {
    let mut d = Document::new();
    let mut body = vec![para_block(Paragraph::with_text("Intro text.", Default::default()))];
    for (text, level) in heads {
        body.push(para_block(Paragraph::with_text(text, Default::default()).styled(&format!("Heading{level}"))));
        body.push(para_block(Paragraph::with_text("Body text.", Default::default())));
    }
    d.body = body;
    d
}

fn reopen(d: &Document) -> Document {
    io::open_bytes("x.docx", &io::save_bytes("x.docx", d).unwrap()).unwrap()
}

#[track_caller]
fn same(d: &Document) -> Document {
    let back = reopen(d);
    match compare_roundtrip(d, &back, &RoundTripLimits::default()).unwrap() {
        Equivalence::Equivalent(_) => {}
        Equivalence::Different(diff) => panic!("not equivalent: {diff:?}"),
    }
    back
}

/// (style, text) of the body's TOC heading and entries.
fn toc(d: &Document) -> Vec<(String, String)> {
    let Some(start) = d.body.iter().position(|b| b.as_para().is_some_and(|p| p.props.style.as_deref() == Some("TOCHeading"))) else {
        return Vec::new();
    };
    d.body[start..]
        .iter()
        .map_while(|b| b.as_para().filter(|p| p.props.style.as_deref().is_some_and(|s| s.starts_with("TOC"))))
        .map(|p| (p.props.style.clone().unwrap_or_default(), p.plain_text()))
        .collect()
}

fn toc_fields(d: &Document) -> Vec<String> {
    d.body
        .iter()
        .filter_map(|b| b.as_para())
        .flat_map(|p| p.objects.iter())
        .filter_map(|o| match o {
            InlineObject::Field { instr, result, .. } if instr.starts_with("TOC") => Some(format!("field:{result}")),
            InlineObject::FieldStart { instr, .. } if instr.starts_with("TOC") => Some("range".into()),
            _ => None,
        })
        .collect()
}

fn entries(d: &Document) -> Vec<String> {
    toc(d).into_iter().skip(1).map(|(st, t)| format!("{st}:{}", t.split('\t').next().unwrap_or(""))).collect()
}

fn caret(s: &mut Session, block: usize) {
    s.run("caret.set", &json!({"pos": Pos::new(StoryRef::Body, Path::top(block), 0)})).unwrap();
}

fn heading_index(d: &Document, text: &str) -> usize {
    d.body
        .iter()
        .position(|b| b.as_para().is_some_and(|p| p.plain_text() == text && p.props.style.as_deref().is_some_and(|s| s.starts_with("Heading"))))
        .unwrap()
}

#[test]
fn inserted_toc_round_trips_with_entries_links_and_pages() {
    let mut s = Session::new(headed(&[("One", 1), ("Two", 2), ("Three", 3), ("Four", 1)]));
    s.run("references.toc", &json!({})).unwrap();
    assert_eq!(entries(&s.doc), ["TOC1:One", "TOC2:Two", "TOC3:Three", "TOC1:Four"]);
    assert_eq!(toc_fields(&s.doc), ["field:"]);
    // `\h`: entries link to hidden `_Toc` bookmarks around the headings, and carry page numbers.
    let first = s.doc.body.iter().filter_map(|b| b.as_para()).find(|p| p.props.style.as_deref() == Some("TOC1")).unwrap();
    assert!(first.plain_text().ends_with("\t1"));
    assert!(first.props_of_char(0).link.as_deref().is_some_and(|l| l.starts_with("#_Toc")));
    let back = same(&s.doc);
    assert_eq!(toc(&back), toc(&s.doc));
    // Second cycle on the reopened document: edit elsewhere, save again.
    let mut s2 = Session::new(back);
    let last = s2.doc.body.len() - 1;
    s2.run("caret.set", &json!({"pos": Pos::new(StoryRef::Body, Path::top(last), 0)})).unwrap();
    s2.run("text.insert", &json!({"text": "Edited. "})).unwrap();
    same(&s2.doc);
}

#[test]
fn empty_heading_list_round_trips_as_the_empty_field() {
    let mut s = Session::new(headed(&[]));
    s.run("references.toc", &json!({})).unwrap();
    assert_eq!(toc(&s.doc).len(), 1, "heading only");
    assert_eq!(toc_fields(&s.doc), ["field:"]);
    let back = same(&s.doc);
    assert_eq!(toc_fields(&back), ["field:"]);
    // Update with still no headings, and after a heading appears.
    let mut s = Session::new(back);
    s.run("references.updateToc", &json!({})).unwrap();
    same(&s.doc);
    let end = s.doc.body.len() - 1;
    s.doc.body.insert(end, para_block(Paragraph::with_text("Late", Default::default()).styled("Heading1")));
    s.touch();
    s.run("references.updateToc", &json!({})).unwrap();
    assert_eq!(entries(&s.doc), ["TOC1:Late"]);
    same(&s.doc);
}

#[test]
fn update_follows_added_deleted_and_reordered_headings() {
    let mut s = Session::new(headed(&[("Alpha", 1), ("Beta", 2), ("Gamma", 1)]));
    s.run("references.toc", &json!({})).unwrap();
    let mut s = Session::new(same(&s.doc));
    assert_eq!(entries(&s.doc), ["TOC1:Alpha", "TOC2:Beta", "TOC1:Gamma"]);

    // Add a heading after Gamma.
    let at = heading_index(&s.doc, "Gamma") + 1;
    s.doc.body.insert(at, para_block(Paragraph::with_text("Delta", Default::default()).styled("Heading2")));
    s.touch();
    s.run("references.updateToc", &json!({})).unwrap();
    assert_eq!(entries(&s.doc), ["TOC1:Alpha", "TOC2:Beta", "TOC1:Gamma", "TOC2:Delta"]);
    let mut s = Session::new(same(&s.doc));

    // Delete Beta (the heading paragraph).
    let beta = heading_index(&s.doc, "Beta");
    s.doc.remove_block(StoryRef::Body, &Path::top(beta)).unwrap();
    s.touch();
    s.run("references.updateToc", &json!({})).unwrap();
    assert_eq!(entries(&s.doc), ["TOC1:Alpha", "TOC1:Gamma", "TOC2:Delta"]);
    let mut s = Session::new(same(&s.doc));

    // Reorder: move Alpha's heading to the end.
    let alpha = heading_index(&s.doc, "Alpha");
    let block = s.doc.body[alpha].clone();
    s.doc.remove_block(StoryRef::Body, &Path::top(alpha)).unwrap();
    s.doc.body.push(block);
    s.touch();
    s.run("references.updateToc", &json!({})).unwrap();
    assert_eq!(entries(&s.doc), ["TOC1:Gamma", "TOC2:Delta", "TOC1:Alpha"]);
    let back = same(&s.doc);
    // Every entry link still targets an existing bookmark.
    let marks: Vec<String> = back.bookmarks().iter().map(|(n, _)| n.clone()).collect();
    for p in
        back.body.iter().filter_map(|b| b.as_para()).filter(|p| p.props.style.as_deref().is_some_and(|s| s.starts_with("TOC") && s != "TOCHeading"))
    {
        let link = p.props_of_char(0).link.clone().unwrap();
        assert!(marks.contains(&link.trim_start_matches('#').to_string()), "{link}");
    }
}

#[test]
fn nested_levels_follow_the_outline_switch() {
    let mut s = Session::new(headed(&[("H1", 1), ("H2", 2), ("H3", 3), ("H4", 4)]));
    s.run("references.toc", &json!({"levels": 2})).unwrap();
    let e = entries(&s.doc);
    assert!(e.iter().all(|e| e.starts_with("TOC1:") || e.starts_with("TOC2:")), "{e:?}");
    same(&s.doc);
}

#[test]
fn undo_and_redo_restore_the_toc_and_stay_equivalent() {
    let mut s = Session::new(headed(&[("One", 1), ("Two", 2)]));
    let before = s.doc.clone();
    s.run("references.toc", &json!({})).unwrap();
    let after = s.doc.clone();
    assert!(s.undo());
    assert_eq!(toc(&s.doc), toc(&before));
    assert!(toc_fields(&s.doc).is_empty());
    same(&s.doc);
    assert!(s.redo());
    assert_eq!(toc(&s.doc), toc(&after));
    same(&s.doc);
    // Update is one undo step too.
    let at = heading_index(&s.doc, "Two") + 1;
    s.run("caret.set", &json!({"pos": Pos::new(StoryRef::Body, Path::top(at), 0)})).unwrap();
    s.run("text.newParagraph", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "Extra"})).unwrap();
    let edited = s.doc.clone();
    s.run("references.updateToc", &json!({})).unwrap();
    assert!(s.undo());
    assert_eq!(toc(&s.doc), toc(&edited));
    assert!(s.redo());
    same(&s.doc);
}

#[test]
fn inserting_into_a_paragraph_middle_and_after_a_table() {
    let mut d = headed(&[("One", 1)]);
    let mut s = Session::new(d.clone());
    s.run("caret.set", &json!({"pos": Pos::new(StoryRef::Body, Path::top(0), 5)})).unwrap();
    s.run("references.toc", &json!({})).unwrap();
    same(&s.doc);
    // A table directly before the insertion point.
    d.body.insert(0, Session::new(Document::new()).doc.body[0].clone());
    let mut s = Session::new(d);
    s.run("table.insert", &json!({"rows": 2, "cols": 2})).ok();
    let last = s.doc.body.len() - 1;
    caret(&mut s, last);
    s.run("references.toc", &json!({})).unwrap();
    same(&s.doc);
}

#[test]
fn imported_word_style_toc_keeps_its_representation_and_updates() {
    // A Word-like TOC: heading, then a range field opening in the first entry and closing at
    // the end of the last one, entries linked to bookmarks.
    let mut s = Session::new(headed(&[("One", 1), ("Two", 1)]));
    s.run("references.toc", &json!({})).unwrap();
    let file = io::save_bytes("x.docx", &s.doc).unwrap();
    let mut imported = Session::new(io::open_bytes("x.docx", &file).unwrap());
    assert_eq!(toc(&imported.doc), toc(&s.doc));
    imported.run("references.updateToc", &json!({})).unwrap();
    assert_eq!(toc(&imported.doc), toc(&s.doc));
    same(&imported.doc);
    // A static TOCHeading followed by entries that are not the field's result keeps its range.
    let range_only =
        |d: &Document| d.body.iter().filter_map(|b| b.as_para()).any(|p| p.objects.iter().any(|o| matches!(o, InlineObject::FieldStart { .. })));
    let mut d = headed(&[("One", 1)]);
    let mut head = Paragraph::with_text("Contents", Default::default()).styled("TOCHeading");
    let end = head.len();
    head.insert_object(end, InlineObject::FieldStart { instr: "TOC \\o \"1-3\"".into(), locked: false }, &Default::default()).unwrap();
    let end = head.len();
    head.insert_object(end, InlineObject::FieldEnd, &Default::default()).unwrap();
    d.body.insert(0, para_block(head));
    d.body.insert(1, para_block(Paragraph::with_text("Static entry", Default::default()).styled("TOC1")));
    let back = reopen(&d);
    assert!(range_only(&back), "a static entry after the heading keeps the explicit range");
    assert!(matches!(&*back.body[1], Block::Para(p) if p.plain_text() == "Static entry"));
}
