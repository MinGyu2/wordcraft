//! Bookmark ownership shared by revision resolution and generated TOC refresh.
use std::collections::{BTreeMap, BTreeSet};

use wordcraft_doc::para::{InlineObject, OBJ};
use wordcraft_doc::{Document, Paragraph, StoryRef};

use crate::CmdError;

/// Validate identities and ordering before mutation, and again before a revision
/// resolution is committed. Ranges may cross paragraphs; they need not nest.
/// TOC refresh may replace obsolete entry targets, so it checks targets only after
/// regenerating its own entries. Marker pairing is always checked before editing.
pub(super) fn validate(doc: &Document, check_targets: bool) -> Result<(), CmdError> {
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
    if check_targets {
        // Generated TOC links must still have a surviving destination. Resolution of
        // an entire referenced range remains unsupported until the TOC is refreshed.
        for para in doc.body.iter().filter_map(|b| b.as_para()) {
            let toc = para
                .props
                .style
                .as_deref()
                .is_some_and(|s| matches!(s.strip_prefix("TOC"), Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")));
            if toc
                && para
                    .runs
                    .iter()
                    .filter_map(|r| r.props.link.as_deref())
                    .any(|link| link.starts_with("#_Toc") && !names.contains(link.trim_start_matches('#')))
            {
                return Err(CmdError::Failed("TOC bookmark target is missing".into()));
            }
        }
    }
    Ok(())
}

/// Partial deletion keeps the range attached to its surviving content. Retain a
/// disappearing boundary when its paragraph-local partner survives this command.
/// Both boundaries inside the deleted revision disappear together. Unknown or
/// cross-paragraph ownership is left to post-validation and atomic rollback.
pub(super) fn retain_partial(para: &mut Paragraph, from: usize, to: usize, accept: bool) -> Result<(), CmdError> {
    let mut pairs = BTreeMap::<String, (Vec<usize>, Vec<usize>)>::new();
    for off in para.object_offsets() {
        match para.object_at(off) {
            Some(InlineObject::BookmarkStart { name }) => pairs.entry(name.clone()).or_default().0.push(off),
            Some(InlineObject::BookmarkEnd { name }) => pairs.entry(name.clone()).or_default().1.push(off),
            _ => {}
        }
    }
    let removing = |off: usize| {
        let props = para.props_of_char(off);
        off >= from && off.saturating_add(OBJ.len_utf8()) <= to && ((accept && props.del.is_some()) || (!accept && props.ins.is_some()))
    };
    let mut keep = Vec::new();
    for (starts, ends) in pairs.values() {
        if let ([start], [end]) = (starts.as_slice(), ends.as_slice())
            && start < end
            && removing(*start) != removing(*end)
        {
            keep.push(if removing(*start) { *start } else { *end });
        }
    }
    for off in keep {
        para.format(off, off + OBJ.len_utf8(), &|c| {
            c.ins = None;
            c.del = None;
        })?;
    }
    Ok(())
}
