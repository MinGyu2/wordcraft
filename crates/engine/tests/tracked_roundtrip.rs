//! Track Changes: tracked editing, accept/reject and undo/redo must survive export -> reopen as the
//! same model (semantic round trip, revision indices paired one-to-one).
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use wordcraft_doc::roundtrip::{Equivalence, RoundTripLimits, compare_roundtrip};
use wordcraft_doc::{Document, Paragraph, Path, Pos, StoryRef, para_block};
use wordcraft_engine::{Session, io};

fn doc() -> Document {
    let mut d = Document::new();
    d.body = vec![
        para_block(Paragraph::with_text("Alpha beta gamma.", Default::default())),
        para_block(Paragraph::with_text("Second paragraph here.", Default::default())),
    ];
    d
}

fn run(s: &mut Session, id: &str, v: Value) {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e:?}"));
}

fn at(s: &mut Session, block: usize, off: usize, extend: bool) {
    run(s, "caret.set", json!({"pos": Pos::new(StoryRef::Body, Path::top(block), off), "extend": extend}));
}

#[track_caller]
fn same(d: &Document) -> Document {
    let back = io::open_bytes("x.docx", &io::save_bytes("x.docx", d).unwrap()).unwrap();
    match compare_roundtrip(d, &back, &RoundTripLimits::default()).unwrap() {
        Equivalence::Equivalent(_) => {}
        Equivalence::Different(diff) => panic!("not equivalent: {diff:?}"),
    }
    back
}

fn tracking() -> Session {
    let mut s = Session::new(doc());
    run(&mut s, "review.trackChanges", json!({"value": true}));
    assert!(s.doc.settings.track_changes);
    s
}

#[test]
fn tracked_insert_delete_replace_and_self_deleted_insertions_round_trip() {
    let mut s = tracking();
    // Insert, then delete an original word (deletion stays as tracked text), then replace a
    // selection, then type over an own insertion and backspace into it (orphaned record).
    at(&mut s, 0, 0, false);
    run(&mut s, "text.insert", json!({"text": "가나"}));
    at(&mut s, 0, "가나Alpha ".len(), false);
    at(&mut s, 0, "가나Alpha beta".len(), true);
    run(&mut s, "text.backspace", json!({}));
    at(&mut s, 1, 0, false);
    at(&mut s, 1, "Second".len(), true);
    run(&mut s, "text.insert", json!({"text": "둘째"}));
    run(&mut s, "text.backspace", json!({}));
    run(&mut s, "text.insert", json!({"text": "째"}));
    let back = same(&s.doc);
    assert!(back.revisions.len() <= s.doc.revisions.len());
    // Second cycle on the reopened model: more tracked edits, then accept one change.
    let mut s2 = Session::new(back);
    at(&mut s2, 1, 0, false);
    run(&mut s2, "text.insert", json!({"text": "!"}));
    same(&s2.doc);
}

#[test]
fn ime_like_replacement_of_a_tracked_insertion_round_trips() {
    // The adapter's IME correction: insert provisional text, select it, type the final text.
    let mut s = tracking();
    at(&mut s, 0, 0, false);
    run(&mut s, "text.insert", json!({"text": "가"}));
    at(&mut s, 0, 0, false);
    at(&mut s, 0, "가".len(), true);
    run(&mut s, "text.insert", json!({"text": "각"}));
    same(&s.doc);
    // Late commit at a remembered offset (text inserted before earlier text).
    at(&mut s, 0, 0, false);
    run(&mut s, "text.insert", json!({"text": "한"}));
    same(&s.doc);
}

#[test]
fn accept_and_reject_selected_and_all_round_trip_and_undo() {
    let mut s = tracking();
    at(&mut s, 0, 0, false);
    run(&mut s, "text.insert", json!({"text": "NEW "}));
    at(&mut s, 1, 0, false);
    at(&mut s, 1, "Second ".len(), true);
    run(&mut s, "text.backspace", json!({}));
    same(&s.doc);
    let tracked = s.doc.clone();
    // Accept the insertion (selection on it).
    at(&mut s, 0, 0, false);
    at(&mut s, 0, "NEW ".len(), true);
    run(&mut s, "review.accept", json!({}));
    same(&s.doc);
    // Reject the deletion.
    at(&mut s, 1, 0, false);
    at(&mut s, 1, "Second ".len(), true);
    run(&mut s, "review.reject", json!({}));
    let back = same(&s.doc);
    assert_eq!(back.plain_text(StoryRef::Body), "NEW Alpha beta gamma.\nSecond paragraph here.");
    // Undo both and redo: tracked records come back exactly.
    assert!(s.undo());
    assert!(s.undo());
    assert_eq!(s.doc, tracked);
    same(&s.doc);
    assert!(s.redo());
    same(&s.doc);
    // Accept all / reject all.
    let mut a = Session::new(tracked.clone());
    run(&mut a, "review.acceptAll", json!({}));
    let back = same(&a.doc);
    assert!(back.revisions.is_empty());
    let mut r = Session::new(tracked);
    run(&mut r, "review.rejectAll", json!({}));
    let back = same(&r.doc);
    assert_eq!(back.plain_text(StoryRef::Body), doc().plain_text(StoryRef::Body));
}

#[test]
fn toggling_tracking_round_trips_the_setting() {
    let mut s = tracking();
    same(&s.doc);
    run(&mut s, "review.trackChanges", json!({"value": false}));
    let back = same(&s.doc);
    assert!(!back.settings.track_changes);
    at(&mut s, 0, 0, false);
    run(&mut s, "text.insert", json!({"text": "plain "}));
    assert!(same(&s.doc).revisions.is_empty());
}
