//! Real engine evidence for `wordcraft_doc::roundtrip`: documents built with Session commands,
//! written to DOCX, read back, and compared. The permutation cases are the ones the derived
//! `Document` equality rejects only because the reader allocates part ids in a different order.

use serde_json::json;
use wordcraft_doc::roundtrip::{Equivalence, RoundTripLimits, compare_roundtrip, roundtrip_equivalent};
use wordcraft_doc::{Block, Document, InlineObject, Part, PartKind, Path, Pos, StoryRef};

use crate::{Session, io};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==";

fn session() -> Session {
    let mut s = Session::new(Document::new());
    s.run("document.setText", &json!({"text": "Alpha beta gamma\nSecond paragraph"})).unwrap();
    s
}

fn header(s: &mut Session, text: &str, picture: bool) {
    s.run("insert.header", &json!({"text": text})).unwrap();
    if picture {
        s.run("insert.picture", &json!({"data": PNG})).unwrap();
    }
    s.run("insert.closeHeader", &json!({})).unwrap();
}

fn footer(s: &mut Session, text: &str, picture: bool) {
    s.run("insert.footer", &json!({"text": text})).unwrap();
    if picture {
        s.run("insert.picture", &json!({"data": PNG})).unwrap();
    }
    s.run("insert.closeHeader", &json!({})).unwrap();
}

fn body_picture(s: &mut Session) {
    s.run("caret.set", &json!({"pos": Pos::body(1, 6)})).unwrap();
    s.run("insert.picture", &json!({"data": PNG})).unwrap();
}

fn comment(s: &mut Session, start: usize, end: usize, text: &str) {
    let pos = |off| Pos::new(StoryRef::Body, Path(vec![0]), off);
    s.run("select.range", &json!({"anchor": pos(start), "focus": pos(end)})).unwrap();
    s.run("review.newComment", &json!({"text": text})).unwrap();
}

fn reopen(doc: &Document) -> Document {
    io::open_bytes("t.docx", &io::save_bytes("t.docx", doc).unwrap()).unwrap()
}

/// Model → export → reopen → API; then the reopened model is exported again (a second save)
/// and must stay equivalent to the authoritative model. Returns (reopened, derived_eq).
fn roundtrip(doc: &Document) -> (Document, bool) {
    let before = doc.clone();
    let reopened = reopen(doc);
    let derived = *doc == reopened;
    let v = compare_roundtrip(doc, &reopened, &RoundTripLimits::default());
    assert!(matches!(v, Ok(Equivalence::Equivalent(_))), "{v:?}");
    let second = reopen(&reopened);
    assert!(roundtrip_equivalent(doc, &second).unwrap(), "second save");
    assert!(roundtrip_equivalent(&reopened, &second).unwrap());
    assert_eq!(*doc, before, "the comparison doesn't mutate its input");
    (reopened, derived)
}

fn has_image(blocks: &[std::sync::Arc<Block>]) -> bool {
    blocks.iter().any(|b| matches!(&**b, Block::Para(p) if p.objects.iter().any(|o| matches!(o, InlineObject::Image { .. }))))
}

/// P3–P9 plus the header-free baselines, after one new comment (geul's B-matrix shapes).
#[test]
fn p3_to_p9_header_footer_image_comment_matrix() {
    for (name, h, f, bi, hi, fi, permuted) in [
        ("plain", false, false, false, false, false, false),
        ("P3 header", true, false, false, false, false, true),
        ("P4 footer", false, true, false, false, false, true),
        ("P5 header+footer", true, true, false, false, false, true),
        ("P6 body image", false, false, true, false, false, false),
        ("P7 header image", true, false, false, true, false, true),
        ("P8 footer image", false, true, false, false, true, true),
        ("P9 header+footer+body image", true, true, true, false, false, true),
        ("header+footer images+body image", true, true, true, true, true, true),
    ] {
        let mut s = session();
        if h {
            header(&mut s, "Header words", hi);
        }
        if f {
            footer(&mut s, "Footer words", fi);
        }
        if bi {
            body_picture(&mut s);
        }
        // Loaded document first (as geul does), then the edit.
        let mut s = Session::new(reopen(&s.doc));
        comment(&mut s, 0, 5, "matrix comment");
        assert_eq!(s.doc.comments.len(), 1, "{name}");
        assert_eq!(has_image(&s.doc.body), bi, "{name}");
        if let Some(id) = s.doc.last_section.headers.default {
            assert_eq!(has_image(&s.doc.parts[&id].blocks), hi, "{name}");
        }
        if let Some(id) = s.doc.last_section.footers.default {
            assert_eq!(has_image(&s.doc.parts[&id].blocks), fi, "{name}");
        }
        let (_, derived) = roundtrip(&s.doc);
        assert_eq!(derived, !permuted, "{name}: derived equality sees only the id permutation");
    }
}

#[test]
fn p10_two_sequential_comments_on_a_rich_document() {
    let mut s = session();
    header(&mut s, "Header words", true);
    footer(&mut s, "Footer words", false);
    body_picture(&mut s);
    let mut s = Session::new(reopen(&s.doc));
    comment(&mut s, 0, 5, "first");
    let (reopened, derived) = roundtrip(&s.doc);
    assert!(!derived);
    // A second comment on the reopened (published) model, then the cycle again.
    let mut s = Session::new(reopened);
    comment(&mut s, 6, 10, "second");
    assert_eq!(s.doc.comments.len(), 2);
    roundtrip(&s.doc);
}

/// Distinct header/footer stories per section (the first section's are on its section break).
#[test]
fn p11_multiple_sections() {
    let mut s = session();
    s.run("caret.set", &json!({"pos": Pos::body(0, 16)})).unwrap();
    s.run("layout.break", &json!({"kind": "nextPage"})).unwrap();
    // `insert.closeHeader` moves the caret, so place it before each story.
    let last = s.doc.body.len() - 1;
    for (block, text, is_header) in
        [(0, "Section one header", true), (0, "Section one footer", false), (last, "Section two header", true), (last, "Section two footer", false)]
    {
        s.run("caret.set", &json!({"pos": Pos::body(block, 0)})).unwrap();
        if is_header {
            header(&mut s, text, false);
        } else {
            footer(&mut s, text, false);
        }
    }
    let first_section = s.doc.body.iter().find_map(|b| b.as_para().and_then(|p| p.section.as_deref())).unwrap();
    assert!(first_section.headers.default.is_some() && first_section.footers.default.is_some());
    assert_ne!(first_section.headers.default, s.doc.last_section.headers.default);
    assert_eq!(s.doc.parts.len(), 4);
    let mut s = Session::new(reopen(&s.doc));
    comment(&mut s, 0, 5, "sections");
    roundtrip(&s.doc);
}

/// First/even/default roles (P12) and two distinct stories with identical text (P14). The pinned
/// commands only create default stories, so the extra roles are set on the model directly.
#[test]
fn p12_p14_first_even_default_roles() {
    let mut s = session();
    header(&mut s, "Same words", false);
    footer(&mut s, "Footer words", false);
    s.run("layout.differentFirstPage", &json!({"value": true})).unwrap();
    s.run("layout.differentOddEven", &json!({"value": true})).unwrap();
    let story = |doc: &mut Document, kind, text: &str| {
        doc.add_part(kind, vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text(text, Default::default()))])
    };
    // An exact copy of the default header's story: same text and style, a distinct part.
    let default_blocks = s.doc.parts[&s.doc.last_section.headers.default.unwrap()].blocks.clone();
    let first = s.doc.add_part(PartKind::Header, default_blocks);
    let even = story(&mut s.doc, PartKind::Header, "Even words");
    let first_footer = story(&mut s.doc, PartKind::Footer, "First footer");
    s.doc.last_section.headers.first = Some(first);
    s.doc.last_section.headers.even = Some(even);
    s.doc.last_section.footers.first = Some(first_footer);
    let reopened = reopen(&s.doc);
    assert!(roundtrip_equivalent(&s.doc, &reopened).unwrap());
    let mut s = Session::new(reopened);
    comment(&mut s, 0, 5, "roles");
    let (reopened, derived) = roundtrip(&s.doc);
    assert!(!derived);
    let (d, f) = (reopened.last_section.headers.default.unwrap(), reopened.last_section.headers.first.unwrap());
    assert_ne!(d, f, "identical text, still two stories");
    assert_eq!(reopened.parts[&d].blocks, reopened.parts[&f].blocks);
}

/// P13: one story referenced by two sections. Records what the pinned DOCX writer/reader do with
/// the alias; the API must report exactly that, never paper over a split.
#[test]
fn p13_shared_story_across_sections() {
    let mut s = session();
    s.run("caret.set", &json!({"pos": Pos::body(0, 16)})).unwrap();
    s.run("layout.break", &json!({"kind": "nextPage"})).unwrap();
    header(&mut s, "Shared header", false);
    let shared = s.doc.last_section.headers.default.unwrap();
    let Some(Block::Para(p)) = s.doc.body.iter_mut().map(std::sync::Arc::make_mut).find(|b| b.as_para().is_some_and(|p| p.section.is_some())) else {
        panic!()
    };
    p.section.as_mut().unwrap().headers.default = Some(shared);
    let reopened = reopen(&s.doc);
    let first = reopened.body.iter().find_map(|b| b.as_para().and_then(|p| p.section.as_deref())).unwrap();
    let aliased = first.headers.default == reopened.last_section.headers.default;
    // Pinned evidence (fc41918): the writer emits one header part and the reader keeps the alias.
    assert!(aliased, "shared story alias survives the round trip");
    assert_eq!(reopened.parts.len(), 1);
    assert!(roundtrip_equivalent(&s.doc, &reopened).unwrap());
    let mut s = Session::new(reopened);
    comment(&mut s, 0, 5, "shared");
    let (reopened, derived) = roundtrip(&s.doc);
    assert!(!derived, "comment renumbers the shared header");
    // Splitting the alias in the reopened model is rejected.
    let mut split = reopened.clone();
    let h = split.last_section.headers.default.unwrap();
    let copy = split.parts[&h].clone();
    let new = split.add_part(PartKind::Header, copy.blocks);
    split.last_section.headers.default = Some(new);
    assert!(!roundtrip_equivalent(&s.doc, &split).unwrap());
}

/// P15: a text box, and a linked pair of text boxes (Create Link), with a comment.
#[test]
fn p15_text_boxes_and_links() {
    let mut s = session();
    header(&mut s, "Header words", false);
    s.run("caret.set", &json!({"pos": Pos::body(1, 0)})).unwrap();
    s.run("insert.textBox", &json!({"text": "Some text that flows on"})).unwrap();
    let simple = reopen(&s.doc);
    assert!(roundtrip_equivalent(&s.doc, &simple).unwrap(), "single text box");
    s.run("caret.set", &json!({"pos": Pos::body(1, 3)})).unwrap();
    s.run("insert.textBox", &json!({})).unwrap();
    s.run("select.objects", &json!({"index": 0})).unwrap();
    s.run("shape.link", &json!({"target": 1})).unwrap();
    assert_eq!(s.doc.text_box_chains().len(), 1);
    let reopened = reopen(&s.doc);
    // Pinned evidence (fc41918): the link (`TextBody::next`) survives DOCX.
    assert_eq!(reopened.text_box_chains().len(), 1);
    assert!(roundtrip_equivalent(&s.doc, &reopened).unwrap());
    let mut s = Session::new(reopened);
    comment(&mut s, 0, 5, "boxes");
    let (reopened, derived) = roundtrip(&s.doc);
    assert!(!derived, "header, two boxes and a comment: ids permuted");
    // Breaking the link in the reopened model is rejected.
    let mut broken = reopened.clone();
    for part in broken.parts.values_mut() {
        part.body.next = None;
    }
    assert!(!roundtrip_equivalent(&s.doc, &broken).unwrap());
}

#[test]
fn footnote_and_endnote_with_comment() {
    let mut s = session();
    header(&mut s, "Header words", false);
    s.run("caret.set", &json!({"pos": Pos::body(0, 5)})).unwrap();
    s.run("references.footnote", &json!({"text": "Note words"})).unwrap();
    s.run("caret.set", &json!({"pos": Pos::body(1, 6)})).unwrap();
    s.run("references.endnote", &json!({"text": "End words"})).unwrap();
    let mut s = Session::new(reopen(&s.doc));
    comment(&mut s, 0, 5, "notes");
    roundtrip(&s.doc);
}

/// Mutations of a real reopened model: each must be rejected although the ids still pair.
#[test]
fn real_reopened_model_rejects_semantic_changes() {
    let mut s = session();
    header(&mut s, "Header words", true);
    footer(&mut s, "Footer words", false);
    body_picture(&mut s);
    let mut s = Session::new(reopen(&s.doc));
    comment(&mut s, 0, 5, "matrix comment");
    let doc = s.doc;
    let base = reopen(&doc);
    assert_ne!(doc, base);
    assert!(roundtrip_equivalent(&doc, &base).unwrap());
    let h = base.last_section.headers.default.unwrap();
    let f = base.last_section.footers.default.unwrap();
    let c = base.comments[&0].part;
    let text = |d: &mut Document, id: u32, t: &str| {
        d.parts.insert(
            id,
            Part { blocks: vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text(t, Default::default()))], ..d.parts[&id].clone() },
        );
    };
    let mutations: Vec<(&str, Box<dyn Fn(&mut Document)>)> = vec![
        ("N1 header text", Box::new(move |d| text(d, h, "Header word"))),
        ("N2 footer text", Box::new(move |d| text(d, f, "Footer word"))),
        (
            "N3 header/footer swapped",
            Box::new(move |d| {
                d.last_section.headers.default = Some(f);
                d.last_section.footers.default = Some(h);
            }),
        ),
        ("N6 kind", Box::new(move |d| d.parts.get_mut(&h).unwrap().kind = PartKind::Footer)),
        ("N11 comment text", Box::new(move |d| text(d, c, "other"))),
        ("N13 comment author", Box::new(|d| d.comments.get_mut(&0).unwrap().author = "Mallory".into())),
        ("N14 comment part", Box::new(move |d| d.comments.get_mut(&0).unwrap().part = h)),
        ("N15 image bytes", Box::new(|d| *d.media.values_mut().next().unwrap() = std::sync::Arc::new(vec![0; 4]))),
        (
            "N16 image size",
            Box::new(|d| {
                for b in d.body.iter_mut() {
                    if let Block::Para(p) = std::sync::Arc::make_mut(b) {
                        for o in &mut p.objects {
                            if let InlineObject::Image { w, .. } = o {
                                *w += 1.0;
                            }
                        }
                    }
                }
            }),
        ),
        ("N4 header removed", Box::new(move |d| drop(d.parts.remove(&h)))),
    ];
    for (name, mutate) in mutations {
        let mut changed = base.clone();
        mutate(&mut changed);
        let v = compare_roundtrip(&doc, &changed, &RoundTripLimits::default());
        assert!(!matches!(v, Ok(Equivalence::Equivalent(_))), "{name}: {v:?}");
    }
}
