//! Synthetic graphs: every expected document is built by hand with explicit part ids (never by
//! relabelling through the implementation's own mapping), then deliberately mutated.

use std::sync::Arc;

use super::*;
use crate::para::{NoteKind, ShapeKind};
use crate::{CharProps, para_block};

fn para(text: &str) -> Arc<Block> {
    para_block(Paragraph::with_text(text, CharProps::default()))
}

fn part(kind: PartKind, text: &str) -> Part {
    Part { kind, blocks: vec![para(text)], body: TextBody::default() }
}

fn text_box(story: u32) -> InlineObject {
    InlineObject::Shape {
        kind: ShapeKind::Rectangle,
        w: 100.0,
        h: 50.0,
        fill: None,
        stroke: None,
        stroke_width: 1.0,
        float: Default::default(),
        story: Some(story),
        freeform: None,
        effects: Default::default(),
        extra: Default::default(),
    }
}

fn image(w: f32) -> InlineObject {
    InlineObject::Image { media: "media/image1.png".into(), w, h: 20.0, alt: String::new(), float: Default::default(), crop: [0.0; 4], ole: None }
}

/// Paragraph `text` with `objects` inserted at the given byte offsets (in order).
fn para_with(text: &str, objects: Vec<(usize, InlineObject)>) -> Arc<Block> {
    let mut p = Paragraph::with_text(text, CharProps::default());
    for (off, o) in objects {
        assert!(p.insert_object(off, o, &CharProps::default()).is_ok());
    }
    para_block(p)
}

/// Explicit part ids of the hand-built rich document.
#[derive(Clone, Copy)]
struct Ids {
    header: u32,
    footer: u32,
    comment: u32,
    note: u32,
    text_box: u32,
}

const A: Ids = Ids { header: 1, footer: 2, comment: 3, note: 4, text_box: 5 };
/// The same graph with every id moved (as a DOCX reader that loads comments first would).
const B: Ids = Ids { header: 4, footer: 5, comment: 1, note: 2, text_box: 3 };

fn rich(ids: Ids) -> Document {
    let mut d = Document::new();
    d.last_section.headers.default = Some(ids.header);
    d.last_section.footers.default = Some(ids.footer);
    d.parts.insert(ids.header, part(PartKind::Header, "Header text"));
    d.parts.insert(ids.footer, part(PartKind::Footer, "Footer text"));
    d.parts.insert(ids.comment, part(PartKind::Comment, "Comment text"));
    d.parts.insert(ids.note, part(PartKind::Footnote, "Note text"));
    d.parts.insert(ids.text_box, part(PartKind::TextBox, "Box text"));
    d.comments.insert(
        0,
        Comment { author: "A".into(), initials: "A".into(), date: "2026-01-01T00:00:00Z".into(), parent: None, resolved: false, part: ids.comment },
    );
    d.media.insert("media/image1.png".into(), Arc::new(vec![1, 2, 3, 4]));
    d.body = vec![
        para_with(
            "Hello world",
            vec![
                (0, InlineObject::CommentStart { id: 0 }),
                (8, InlineObject::CommentEnd { id: 0 }),
                (11, InlineObject::NoteRef { kind: NoteKind::Footnote, id: ids.note, custom: String::new() }),
                (14, text_box(ids.text_box)),
                (17, image(30.0)),
            ],
        ),
        para("Second paragraph"),
    ];
    d
}

fn verdict(a: &Document, b: &Document) -> Result<Equivalence, RoundTripError> {
    compare_roundtrip(a, b, &RoundTripLimits::default())
}

fn different(a: &Document, b: &Document) -> Difference {
    match verdict(a, b) {
        Ok(Equivalence::Different(d)) => d,
        other => panic!("expected a difference, got {other:?}"),
    }
}

fn body_para(d: &mut Document, i: usize) -> &mut Paragraph {
    match Arc::make_mut(&mut d.body[i]) {
        Block::Para(p) => p,
        Block::Table(_) => panic!("not a paragraph"),
    }
}

fn part_text(d: &mut Document, id: u32, text: &str) {
    d.parts.get_mut(&id).unwrap().blocks = vec![para(text)];
}

// ---- positive ----

#[test]
fn p1_identical_document() {
    let a = rich(A);
    let before = a.clone();
    let v = verdict(&a, &a.clone()).unwrap();
    let Equivalence::Equivalent(map) = v else { panic!("{v:?}") };
    assert_eq!(map.iter().collect::<Vec<_>>(), vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);
    assert_eq!(a, before, "inputs are not mutated");
}

#[test]
fn p2_harmless_permutation() {
    let (a, b) = (rich(A), rich(B));
    let (a0, b0) = (a.clone(), b.clone());
    assert_ne!(a, b, "derived equality still sees the id permutation");
    let Equivalence::Equivalent(map) = verdict(&a, &b).unwrap() else { panic!() };
    let expected = [(A.header, B.header), (A.footer, B.footer), (A.comment, B.comment), (A.note, B.note), (A.text_box, B.text_box)];
    let mut expected = expected.to_vec();
    expected.sort();
    assert_eq!(map.iter().collect::<Vec<_>>(), expected);
    assert!(roundtrip_equivalent(&a, &b).unwrap());
    assert!(roundtrip_equivalent(&b, &a).unwrap());
    assert_eq!((a, b), (a0, b0), "inputs are not mutated");
}

/// Shared part referenced from several roles and sections, permuted.
#[test]
fn p13_repeated_references_and_p11_sections() {
    let build = |h: u32, f: u32| {
        let mut d = Document::new();
        d.parts.insert(h, part(PartKind::Header, "H"));
        d.parts.insert(f, part(PartKind::Footer, "F"));
        d.last_section.headers.default = Some(h);
        d.last_section.headers.first = Some(h);
        d.last_section.footers.even = Some(f);
        let mut first = Paragraph::with_text("Section one", CharProps::default());
        let mut s = SectionProps::default();
        s.headers.default = Some(h);
        s.footers.default = Some(f);
        first.section = Some(Box::new(s));
        d.body = vec![para_block(first), para("Section two")];
        d
    };
    let (a, b) = (build(1, 2), build(2, 1));
    assert_ne!(a, b);
    assert!(roundtrip_equivalent(&a, &b).unwrap());
}

/// Two distinct parts with identical text stay two parts.
#[test]
fn p14_identical_content_distinct_parts() {
    let build = |x: u32, y: u32| {
        let mut d = Document::new();
        d.parts.insert(x, part(PartKind::Header, "same"));
        d.parts.insert(y, part(PartKind::Header, "same"));
        d.last_section.headers.default = Some(x);
        d.last_section.headers.even = Some(y);
        d
    };
    let a = build(1, 2);
    let Equivalence::Equivalent(map) = verdict(&a, &build(2, 1)).unwrap() else { panic!() };
    assert_eq!(map.iter().collect::<Vec<_>>(), vec![(1, 2), (2, 1)]);
}

fn chain(ids: [u32; 3], next: [Option<u32>; 3]) -> Document {
    let mut d = Document::new();
    for (id, n) in ids.into_iter().zip(next) {
        let mut p = part(PartKind::TextBox, &format!("box{id}"));
        p.body.next = n;
        d.parts.insert(id, p);
    }
    d.body = vec![para_with("abc", vec![(0, text_box(ids[0])), (4, text_box(ids[1])), (8, text_box(ids[2]))])];
    d
}

/// Linked text boxes, including a cycle, under a permutation; part text is keyed by the
/// authoritative id, so the reopened parts carry the authoritative texts explicitly.
#[test]
fn p15_linked_text_box_cycle() {
    let a = chain([1, 2, 3], [Some(2), Some(3), Some(1)]);
    let mut b = chain([3, 1, 2], [Some(1), Some(2), Some(3)]);
    for (old, new) in [(1, 3), (2, 1), (3, 2)] {
        part_text(&mut b, new, &format!("box{old}"));
    }
    assert!(roundtrip_equivalent(&a, &b).unwrap());
    // E: the cycle's topology must survive (3 → 1 becomes 3 → 3: a self-loop).
    let mut c = b.clone();
    c.parts.get_mut(&2).unwrap().body.next = Some(2);
    assert!(!roundtrip_equivalent(&a, &c).unwrap());
}

#[test]
fn nested_group_text_box_and_table_cell_section() {
    let build = |h: u32, t: u32| {
        let mut d = Document::new();
        d.parts.insert(h, part(PartKind::Header, "H"));
        d.parts.insert(t, part(PartKind::TextBox, "T"));
        let group = InlineObject::Group {
            w: 10.0,
            h: 10.0,
            float: Default::default(),
            ch_w: 10.0,
            ch_h: 10.0,
            children: vec![crate::para::GroupChild {
                x: 0.0,
                y: 0.0,
                obj: InlineObject::Group {
                    w: 5.0,
                    h: 5.0,
                    float: Default::default(),
                    ch_w: 5.0,
                    ch_h: 5.0,
                    children: vec![crate::para::GroupChild { x: 1.0, y: 1.0, obj: text_box(t) }],
                    canvas: None,
                },
            }],
            canvas: None,
        };
        let mut cell_para = Paragraph::with_text("cell", CharProps::default());
        let mut s = SectionProps::default();
        s.headers.default = Some(h);
        cell_para.section = Some(Box::new(s));
        let mut table = Table::default();
        table.rows.push(Row { cells: vec![Cell { blocks: vec![para_block(cell_para)], ..Cell::empty() }], ..Default::default() });
        d.body = vec![para_with("g", vec![(0, group)]), Arc::new(Block::Table(table)), para("")];
        d
    };
    let a = build(1, 2);
    assert!(roundtrip_equivalent(&a, &build(2, 1)).unwrap());
    // N19: nested story reference retargeted to the header (kind) …
    let mut wrong = build(2, 1);
    let Block::Table(t) = Arc::make_mut(&mut wrong.body[1]) else { panic!() };
    let Block::Para(p) = Arc::make_mut(&mut t.rows[0].cells[0].blocks[0]) else { panic!() };
    p.section.as_mut().unwrap().headers.default = None;
    assert_eq!(different(&a, &wrong), Difference::ReferencePresence("header/footer default"));
}

// ---- adversarial ----

fn permuted() -> Document {
    rich(B)
}

#[test]
fn n1_n2_header_footer_text_changed() {
    let a = rich(A);
    let mut b = permuted();
    part_text(&mut b, B.header, "Header texT");
    assert_eq!(different(&a, &b), Difference::Field("paragraph"));
    let mut b = permuted();
    part_text(&mut b, B.footer, "Footer");
    assert_eq!(different(&a, &b), Difference::Field("paragraph"));
}

#[test]
fn n3_header_footer_exchanged_and_n6_kind_changed() {
    let a = rich(A);
    let mut b = permuted();
    b.last_section.headers.default = Some(B.footer);
    b.last_section.footers.default = Some(B.header);
    assert!(matches!(different(&a, &b), Difference::PartKind { .. }));
    let mut b = permuted();
    b.parts.get_mut(&B.header).unwrap().kind = PartKind::Footer;
    assert!(matches!(different(&a, &b), Difference::PartKind { .. }));
}

#[test]
fn n4_n21_referenced_part_removed_or_missing() {
    let a = rich(A);
    let mut b = permuted();
    b.parts.remove(&B.header);
    assert_eq!(different(&a, &b), Difference::Field("parts.len"));
    // Same count, but the header reference dangles.
    let mut b = permuted();
    let header = b.parts.remove(&B.header).unwrap();
    b.parts.insert(99, header);
    assert_eq!(verdict(&a, &b), Err(RoundTripError::DanglingReference { side: Side::Reopened, id: B.header }));
    // A dangling authoritative reference is never equivalent, even to itself.
    let mut c = rich(A);
    c.last_section.headers.first = Some(77);
    assert_eq!(verdict(&c, &c.clone()), Err(RoundTripError::DanglingReference { side: Side::Authoritative, id: 77 }));
}

#[test]
fn n5_extra_part_introduced() {
    let a = rich(A);
    let mut b = permuted();
    b.parts.insert(40, part(PartKind::Header, "stray"));
    assert_eq!(different(&a, &b), Difference::Field("parts.len"));
}

/// N7 merge / N8 duplicate / N10 alias change, with identical-looking content throughout.
#[test]
fn n7_n8_n10_alias_changes() {
    let build = |default: u32, first: u32, even: u32| {
        let mut d = Document::new();
        d.parts.insert(1, part(PartKind::Header, "same"));
        d.parts.insert(2, part(PartKind::Header, "same"));
        d.last_section.headers.default = Some(default);
        d.last_section.headers.first = Some(first);
        d.last_section.headers.even = Some(even);
        d
    };
    let a = build(1, 1, 2); // default and first share part 1; even is distinct.
    assert!(roundtrip_equivalent(&a, &build(2, 2, 1)).unwrap());
    // N8: the shared part is duplicated (first now distinct from default).
    assert_eq!(different(&a, &build(1, 2, 2)), Difference::Split { authoritative: 1 });
    // N7: the distinct parts are merged (even now shares default's part).
    assert_eq!(different(&a, &build(1, 1, 1)), Difference::Merge { reopened: 1 });
    let mut n8 = rich(A);
    n8.last_section.headers.first = Some(A.header);
    let mut dup = rich(B);
    dup.parts.insert(9, part(PartKind::Header, "Header text"));
    dup.last_section.headers.first = Some(9);
    assert!(!roundtrip_equivalent(&n8, &dup).unwrap());
}

#[test]
fn n9_reference_points_to_a_different_part() {
    let build = |d1: u32, f1: u32| {
        let mut d = Document::new();
        d.parts.insert(1, part(PartKind::Header, "H1"));
        d.parts.insert(2, part(PartKind::Header, "H2"));
        d.last_section.headers.default = Some(d1);
        d.last_section.headers.first = Some(f1);
        d
    };
    assert!(!roundtrip_equivalent(&build(1, 2), &build(2, 1)).unwrap());
}

#[test]
fn n11_to_n14_comments() {
    let a = rich(A);
    // N11 text.
    let mut b = permuted();
    part_text(&mut b, B.comment, "Comment texts");
    assert_eq!(different(&a, &b), Difference::Field("paragraph"));
    // N12 anchor moved.
    let mut b = permuted();
    let p = body_para(&mut b, 0);
    let start = p.text.find(crate::para::OBJ).unwrap();
    p.text.replace_range(start..start + 3, "");
    p.text.insert(1, crate::para::OBJ);
    assert_eq!(different(&a, &b), Difference::Field("paragraph"));
    // N13 identity changed (key and anchors).
    let mut b = permuted();
    let c = b.comments.remove(&0).unwrap();
    b.comments.insert(5, c);
    for o in &mut body_para(&mut b, 0).objects {
        if let InlineObject::CommentStart { id } | InlineObject::CommentEnd { id } = o {
            *id = 5;
        }
    }
    assert_eq!(different(&a, &b), Difference::Field("comment id"));
    // N14 two comments; the second's part reference collapses onto the first's.
    let mut a2 = rich(A);
    a2.parts.insert(6, part(PartKind::Comment, "Second comment"));
    a2.comments.insert(1, Comment { part: 6, ..a2.comments[&0].clone() });
    let mut b2 = permuted();
    b2.parts.insert(6, part(PartKind::Comment, "Second comment"));
    b2.comments.insert(1, Comment { part: B.comment, ..b2.comments[&0].clone() });
    assert_eq!(different(&a2, &b2), Difference::Merge { reopened: B.comment });
    let mut ok = permuted();
    ok.parts.insert(6, part(PartKind::Comment, "Second comment"));
    ok.comments.insert(1, Comment { part: 6, ..ok.comments[&0].clone() });
    assert!(roundtrip_equivalent(&a2, &ok).unwrap());
}

#[test]
fn n15_n16_image_bytes_and_dimensions() {
    let a = rich(A);
    let mut b = permuted();
    b.media.insert("media/image1.png".into(), Arc::new(vec![1, 2, 3, 5]));
    assert_eq!(different(&a, &b), Difference::Field("media"));
    let mut b = permuted();
    let p = body_para(&mut b, 0);
    let last = p.objects.len() - 1;
    p.objects[last] = image(31.0);
    assert_eq!(different(&a, &b), Difference::Field("inline object"));
}

#[test]
fn n17_inline_object_moved() {
    let a = rich(A);
    let mut b = permuted();
    let p = body_para(&mut b, 0);
    let n = p.objects.len();
    p.objects.swap(n - 1, n - 2); // image and text box exchange places.
    assert_eq!(different(&a, &b), Difference::Field("inline object kind"));
    // Moved to the other paragraph.
    let mut b = permuted();
    let image = body_para(&mut b, 0).objects.pop().unwrap();
    let p = body_para(&mut b, 1);
    p.objects.push(image);
    assert_eq!(different(&a, &b), Difference::Field("object count"));
}

#[test]
fn n18_section_reference_changed() {
    let build = |inner: u32| {
        let mut d = Document::new();
        d.parts.insert(1, part(PartKind::Header, "H"));
        d.parts.insert(2, part(PartKind::Header, "H"));
        d.last_section.headers.default = Some(1);
        let mut p = Paragraph::with_text("s1", CharProps::default());
        let mut s = SectionProps::default();
        s.headers.default = Some(inner);
        p.section = Some(Box::new(s));
        d.body = vec![para_block(p), para("s2")];
        d
    };
    // Section 1's own header (2) now aliases the last section's (1); part 2 becomes unreached.
    let a = build(2);
    assert!(!matches!(verdict(&a, &build(1)), Ok(Equivalence::Equivalent(_))));
    // Tracked section change (`fmt_change.old`) carries references too.
    let mut x = build(2);
    let mut y = build(2);
    let mut old = SectionProps::default();
    old.headers.default = Some(1);
    if let Block::Para(p) = Arc::make_mut(&mut x.body[0]) {
        p.section.as_mut().unwrap().fmt_change = PropChange::boxed(0, old.clone());
    }
    old.headers.default = Some(2);
    if let Block::Para(p) = Arc::make_mut(&mut y.body[0]) {
        p.section.as_mut().unwrap().fmt_change = PropChange::boxed(0, old);
    }
    assert!(roundtrip_equivalent(&x, &x.clone()).unwrap());
    // last section pairs 1↔1 first, so the tracked change's 1↔2 is a split.
    assert_eq!(different(&x, &y), Difference::Split { authoritative: 1 });
}

#[test]
fn n19_nested_story_reference_changed() {
    let mut a = rich(A);
    a.parts.insert(6, part(PartKind::TextBox, "inner"));
    a.parts.get_mut(&A.header).unwrap().blocks = vec![para_with("Header text", vec![(0, text_box(6))])];
    let mut b = permuted();
    b.parts.insert(6, part(PartKind::TextBox, "inner"));
    b.parts.get_mut(&B.header).unwrap().blocks = vec![para_with("Header text", vec![(0, text_box(6))])];
    assert!(roundtrip_equivalent(&a, &b).unwrap());
    // Retarget the header's text box to the body's text box story.
    let mut c = b.clone();
    c.parts.get_mut(&B.header).unwrap().blocks = vec![para_with("Header text", vec![(0, text_box(B.text_box))])];
    assert!(!roundtrip_equivalent(&a, &c).unwrap());
}

#[test]
fn n20_invalid_unsupported_structures() {
    // A header role pointing at a comment part.
    let mut a = rich(A);
    a.last_section.headers.first = Some(A.comment);
    assert!(matches!(verdict(&a, &a.clone()), Err(RoundTripError::UnexpectedPartKind { side: Side::Authoritative, .. })));
    // A part no stable root reaches can't be paired.
    let mut a = rich(A);
    a.parts.insert(30, part(PartKind::Footnote, "orphan"));
    assert_eq!(verdict(&a, &a.clone()), Err(RoundTripError::UnreachablePart { id: 30 }));
    // Self-linked box (cycle of one) is fine when both sides agree.
    let a = chain([1, 2, 3], [Some(1), None, None]);
    assert!(roundtrip_equivalent(&a, &a.clone()).unwrap());
}

/// N22: a full, valid permutation with one deep change (the footnote text).
#[test]
fn n22_difference_hidden_behind_permutation() {
    let a = rich(A);
    let mut b = permuted();
    assert!(roundtrip_equivalent(&a, &b).unwrap());
    part_text(&mut b, B.note, "Note text.");
    assert!(!roundtrip_equivalent(&a, &b).unwrap());
    let mut b = permuted();
    part_text(&mut b, B.text_box, "Box");
    assert!(!roundtrip_equivalent(&a, &b).unwrap());
}

#[test]
fn limits_are_errors_never_equivalence() {
    let a = rich(A);
    let tight = RoundTripLimits { max_steps: 3, ..Default::default() };
    assert_eq!(compare_roundtrip(&a, &a.clone(), &tight), Err(RoundTripError::LimitExceeded));
    // Group nesting deeper than the limit (built iteratively).
    let mut obj = text_box(1);
    for _ in 0..40 {
        obj = InlineObject::Group {
            w: 1.0,
            h: 1.0,
            float: Default::default(),
            ch_w: 1.0,
            ch_h: 1.0,
            children: vec![crate::para::GroupChild { x: 0.0, y: 0.0, obj }],
            canvas: None,
        };
    }
    let mut d = Document::new();
    d.parts.insert(1, part(PartKind::TextBox, "deep"));
    d.body = vec![para_with("x", vec![(0, obj)])];
    assert!(roundtrip_equivalent(&d, &d.clone()).unwrap());
    let shallow = RoundTripLimits { max_depth: 10, ..Default::default() };
    assert_eq!(compare_roundtrip(&d, &d.clone(), &shallow), Err(RoundTripError::LimitExceeded));
}

/// Audit gap: a text box reference inside a nested group, and a note reference whose part kind
/// is substituted (footnote ↔ endnote), both under an otherwise valid permutation.
#[test]
fn nested_group_retarget_and_note_kind_substitution() {
    use crate::para::GroupChild;
    let build = |inner: u32, outer: u32| {
        let mut d = Document::new();
        d.parts.insert(1, part(PartKind::TextBox, "same"));
        d.parts.insert(2, part(PartKind::TextBox, "same"));
        let nested = InlineObject::Group {
            w: 5.0,
            h: 5.0,
            float: Default::default(),
            ch_w: 5.0,
            ch_h: 5.0,
            children: vec![GroupChild { x: 1.0, y: 1.0, obj: text_box(inner) }],
            canvas: None,
        };
        let group = InlineObject::Group {
            w: 10.0,
            h: 10.0,
            float: Default::default(),
            ch_w: 10.0,
            ch_h: 10.0,
            children: vec![GroupChild { x: 0.0, y: 0.0, obj: nested }, GroupChild { x: 2.0, y: 2.0, obj: text_box(outer) }],
            canvas: None,
        };
        d.body = vec![para_with("g", vec![(0, group)])];
        d
    };
    let a = build(1, 2);
    assert!(roundtrip_equivalent(&a, &build(2, 1)).unwrap(), "permuted, identical-text boxes");
    // The nested member now shows the outer member's story: two distinct stories merged.
    assert_eq!(different(&a, &build(2, 2)), Difference::Merge { reopened: 2 });

    let notes = |foot: u32, end: u32, foot_kind: PartKind, end_kind: PartKind| {
        let mut d = Document::new();
        d.parts.insert(foot, part(foot_kind, "note"));
        d.parts.insert(end, part(end_kind, "note"));
        d.body = vec![para_with(
            "ab",
            vec![
                (0, InlineObject::NoteRef { kind: NoteKind::Footnote, id: foot, custom: String::new() }),
                (4, InlineObject::NoteRef { kind: NoteKind::Endnote, id: end, custom: String::new() }),
            ],
        )];
        d
    };
    let a = notes(1, 2, PartKind::Footnote, PartKind::Endnote);
    assert!(roundtrip_equivalent(&a, &notes(2, 1, PartKind::Footnote, PartKind::Endnote)).unwrap());
    // Same ids and text, kinds exchanged in the reopened document.
    assert!(matches!(different(&a, &notes(1, 2, PartKind::Endnote, PartKind::Footnote)), Difference::PartKind { .. }));
    // An authoritative footnote reference to an endnote part can't be proven at all.
    let bad = notes(1, 2, PartKind::Endnote, PartKind::Endnote);
    assert!(matches!(verdict(&bad, &bad.clone()), Err(RoundTripError::UnexpectedPartKind { .. })));
}

/// Audit gap (2nd pass): depth limits for table nesting and tracked section-change chains are
/// enforced (iteratively), and the reference inside the deepest level is still compared.
#[test]
fn deep_tables_and_section_change_chains_are_bounded() {
    // A table nested 20 deep; the innermost cell's section references header part `h`.
    let tables = |h: u32| {
        let mut p = Paragraph::with_text("deep", CharProps::default());
        let mut s = SectionProps::default();
        s.headers.default = Some(h);
        p.section = Some(Box::new(s));
        let mut blocks = vec![para_block(p)];
        for _ in 0..20 {
            let mut t = Table::default();
            t.rows.push(Row { cells: vec![Cell { blocks, ..Cell::empty() }], ..Default::default() });
            blocks = vec![Arc::new(Block::Table(t)), para("")];
        }
        let mut d = Document::new();
        d.parts.insert(1, part(PartKind::Header, "H1"));
        d.parts.insert(2, part(PartKind::Header, "H2"));
        d.last_section.headers.default = Some(if h == 1 { 2 } else { 1 });
        d.body = blocks;
        d
    };
    let a = tables(1);
    assert!(roundtrip_equivalent(&a, &a.clone()).unwrap());
    // The innermost reference points at the other part: detected 20 levels down.
    assert!(!roundtrip_equivalent(&a, &tables(2)).unwrap());
    let shallow = RoundTripLimits { max_depth: 5, ..Default::default() };
    assert_eq!(compare_roundtrip(&a, &a.clone(), &shallow), Err(RoundTripError::LimitExceeded));

    // A tracked section-change chain 20 long; the oldest entry references a header.
    let chain = |h: u32| {
        let mut old = SectionProps::default();
        old.headers.default = Some(h);
        for rev in 0..20 {
            old = SectionProps { fmt_change: PropChange::boxed(rev, old), ..Default::default() };
        }
        let mut d = Document::new();
        d.parts.insert(1, part(PartKind::Header, "H"));
        d.last_section = old;
        d
    };
    let a = chain(1);
    assert!(roundtrip_equivalent(&a, &a.clone()).unwrap());
    assert_eq!(compare_roundtrip(&a, &a.clone(), &shallow), Err(RoundTripError::LimitExceeded));
    let mut b = chain(1);
    b.parts.insert(9, b.parts[&1].clone());
    b.parts.remove(&1);
    assert!(matches!(verdict(&a, &b), Err(RoundTripError::DanglingReference { side: Side::Reopened, id: 1 })));
}

/// `u32` identifiers that are not part ids are compared by value, never remapped: a permutation
/// of part ids must not excuse a changed comment parent, drawing id or permission id.
#[test]
fn non_part_identifiers_are_not_remapped() {
    let a = rich(A);
    let mut b = permuted();
    b.comments.get_mut(&0).unwrap().parent = Some(3);
    assert_eq!(different(&a, &b), Difference::Field("comment"));
    let mut b = permuted();
    for o in &mut body_para(&mut b, 0).objects {
        if let InlineObject::Shape { extra, .. } = o {
            extra.id = 7;
        }
    }
    assert_eq!(different(&a, &b), Difference::Field("shape"));
    let with_perm = |d: &mut Document, id: u32| {
        let p = body_para(d, 1);
        assert!(p.insert_object(0, InlineObject::PermStart { id, group: "everyone".into(), editor: String::new() }, &CharProps::default()).is_ok());
    };
    let mut a2 = rich(A);
    with_perm(&mut a2, 1);
    let mut b2 = permuted();
    with_perm(&mut b2, 1);
    assert!(roundtrip_equivalent(&a2, &b2).unwrap());
    let mut b3 = permuted();
    with_perm(&mut b3, 4); // 4 is A.note: a valid *part* id, but not this permission range.
    assert_eq!(different(&a2, &b3), Difference::Field("inline object"));
}
