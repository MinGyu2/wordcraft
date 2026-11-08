//! Bookmark ownership shared by revision resolution and generated TOC refresh.
use std::collections::{BTreeMap, BTreeSet};

use wordcraft_doc::para::{InlineObject, OBJ};
use wordcraft_doc::{Document, Path, StoryRef};

use crate::{CmdError, Session};

/// Validate identities and ordering before mutation, and again before a revision
/// resolution is committed. Ranges may cross paragraphs; they need not nest.
/// TOC refresh may replace obsolete entry targets, so it checks targets only after
/// regenerating its own entries. Marker pairing is always checked before editing.
pub(super) fn validate(doc: &Document, check_targets: bool) -> Result<(), CmdError> {
    let names = names(doc)?;
    if check_targets && toc_target_missing(doc, &names) {
        return Err(CmdError::Failed("TOC bookmark target is missing".into()));
    }
    Ok(())
}

/// Every bookmark name, after checking identities and pairing in every story.
pub(super) fn names(doc: &Document) -> Result<BTreeSet<String>, CmdError> {
    let mut names = BTreeSet::new();
    for story in std::iter::once(StoryRef::Body).chain(doc.parts.keys().map(|id| StoryRef::Part(*id))) {
        let mut open = BTreeSet::new();
        for path in doc.para_paths(story) {
            let Some(para) = doc.para(story, &path) else {
                continue;
            };
            for off in para.object_offsets() {
                match para.object_at(off) {
                    Some(InlineObject::BookmarkStart { name }) => {
                        if name.is_empty() || !names.insert(name.clone()) || !open.insert(name.clone()) {
                            return Err(CmdError::Failed("ambiguous bookmark start".into()));
                        }
                    }
                    Some(InlineObject::BookmarkEnd { name }) if !open.remove(name) => {
                        return Err(CmdError::Failed("unpaired bookmark end".into()));
                    }
                    _ => {}
                }
            }
        }
        if !open.is_empty() {
            return Err(CmdError::Failed("unpaired bookmark start".into()));
        }
    }
    Ok(names)
}

fn toc_paragraph(para: &wordcraft_doc::Paragraph) -> bool {
    para.props.style.as_deref().is_some_and(|s| matches!(s.strip_prefix("TOC"), Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")))
}

/// Generated TOC links must still have a surviving destination.
fn toc_target_missing(doc: &Document, names: &BTreeSet<String>) -> bool {
    doc.body.iter().filter_map(|b| b.as_para()).filter(|p| toc_paragraph(p)).any(|para| {
        para.runs
            .iter()
            .filter_map(|r| r.props.link.as_deref())
            .any(|link| link.starts_with("#_Toc") && !names.contains(link.trim_start_matches('#')))
    })
}

/// Finish a revision resolution that started with bookmarks `before`:
/// - marker identities and pairing must still hold;
/// - an internal hyperlink outside the TOC whose bookmark this resolution removed would
///   dangle, and nothing regenerates it: refuse (the caller's command rolls back);
/// - a TOC entry whose heading this resolution removed is obsolete: regenerate the TOC
///   from the surviving headings within the same command (one undo step), then require
///   every TOC link to resolve. Without a regenerable TOC field the resolution is refused.
pub(super) fn finish_resolution(s: &mut Session, before: &BTreeSet<String>) -> Result<(), CmdError> {
    let after = names(&s.doc)?;
    let removed: BTreeSet<&String> = before.difference(&after).collect();
    if !removed.is_empty() {
        for story in std::iter::once(StoryRef::Body).chain(s.doc.parts.keys().map(|id| StoryRef::Part(*id))) {
            for path in s.doc.para_paths(story) {
                let Some(para) = s.doc.para(story, &path) else { continue };
                if story == StoryRef::Body && toc_paragraph(para) {
                    continue;
                }
                let dangling = para
                    .runs
                    .iter()
                    .filter_map(|r| r.props.link.as_deref())
                    .filter_map(|link| link.strip_prefix('#'))
                    .any(|name| removed.iter().any(|r| r.as_str() == name));
                if dangling {
                    return Err(CmdError::Failed("internal hyperlink target would be removed".into()));
                }
            }
        }
    }
    if toc_target_missing(&s.doc, &after) {
        super::references::update_toc(s)?;
        validate(&s.doc, true)?;
    }
    Ok(())
}

/// Partial deletion keeps a range attached to its surviving content, in one paragraph or
/// across paragraphs. Before a resolution mutates anything, find the bookmark markers it
/// removes (`spans`: the (paragraph, from, to) ranges it resolves in `story`; a marker goes
/// with an accepted deletion or a rejected insertion). When exactly one boundary of a pair
/// goes and its partner survives, keep the disappearing boundary in place (its revision is
/// cleared). Both boundaries removed: the bookmark disappears with its content. Unpaired or
/// misordered markers are left to post-validation and atomic rollback.
pub(super) fn retain_boundaries(doc: &mut Document, story: StoryRef, spans: &[(Path, usize, usize)], accept: bool) -> Result<(), CmdError> {
    type Marker = (usize, Path, usize, bool);
    let mut pairs = BTreeMap::<String, (Vec<Marker>, Vec<Marker>)>::new();
    for (order, path) in doc.para_paths(story).into_iter().enumerate() {
        let Some(para) = doc.para(story, &path) else { continue };
        let span = spans.iter().find(|(p, ..)| *p == path).map(|(_, from, to)| (*from, *to));
        for off in para.object_offsets() {
            let removing = span.is_some_and(|(from, to)| {
                let props = para.props_of_char(off);
                off >= from && off.saturating_add(OBJ.len_utf8()) <= to && ((accept && props.del.is_some()) || (!accept && props.ins.is_some()))
            });
            match para.object_at(off) {
                Some(InlineObject::BookmarkStart { name }) => pairs.entry(name.clone()).or_default().0.push((order, path.clone(), off, removing)),
                Some(InlineObject::BookmarkEnd { name }) => pairs.entry(name.clone()).or_default().1.push((order, path.clone(), off, removing)),
                _ => {}
            }
        }
    }
    let mut keep = Vec::new();
    for (starts, ends) in pairs.values() {
        if let ([start], [end]) = (starts.as_slice(), ends.as_slice())
            && (start.0, start.2) < (end.0, end.2)
            && start.3 != end.3
        {
            let gone = if start.3 { start } else { end };
            keep.push((gone.1.clone(), gone.2));
        }
    }
    for (path, off) in keep {
        doc.para_mut(story, &path)?.format(off, off + OBJ.len_utf8(), &|c| {
            c.ins = None;
            c.del = None;
        })?;
    }
    Ok(())
}
