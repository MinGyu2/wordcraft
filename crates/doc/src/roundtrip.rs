//! Semantic round-trip equivalence: are two documents the same document, allowing only a
//! permutation of the internal `Document::parts` ids?
//!
//! Part ids are allocation artefacts. A model that gains a comment after its header was created
//! (header = 1, comment = 2) is written to a file and read back with the comment first
//! (comment = 1, header = 2); the derived [`PartialEq`] of [`Document`] then reports a difference
//! that isn't one. This module is a separate, explicitly named contract: it leaves
//! `Document::eq` (undo snapshots, edit history) exactly as it is.
//!
//! # How correspondence is established
//!
//! Both documents are walked in parallel from stable roots — the comments (by comment id), the
//! last section's header/footer roles, the note separators and the body — in document order.
//! Every place that holds a part id (a *reference site*) is paired with the site at the same
//! structural position in the other document, which extends a bidirectional map
//! `authoritative id ↔ reopened id`. A newly paired part is queued, and its own story is walked
//! the same way, so nested and cyclic references (linked text boxes) are followed exactly once.
//! Every non-id field is compared with ordinary equality on the way.
//!
//! The map must stay one-to-one (a split or a merge of shared references is a difference),
//! paired parts must have the same [`PartKind`] (and the kind the site expects), and at the end
//! it must cover every part of both documents. There is no search or backtracking: stable roots
//! determine the correspondence.
//!
//! # Reference sites (pinned model inventory)
//!
//! - `SectionProps::headers` / `footers` ([`HeaderSet`] `default`/`first`/`even`), for
//!   `Document::last_section`, for every `Paragraph::section` (body, parts, table cells) and for
//!   every `SectionProps::fmt_change` `old` section, transitively;
//! - `Comment::part`;
//! - `InlineObject::NoteRef::id` (footnote/endnote story);
//! - `InlineObject::Shape::story` (text box story), also inside `InlineObject::Group` members,
//!   nested groups included;
//! - `TextBody::next` of a part (Create Link continuation box).
//!
//! # Revision indices
//!
//! `Document::revisions` indices (`CharProps::ins`/`del`, every `PropChange::rev` of run,
//! paragraph-mark, paragraph, numbering, table, row, cell and section changes) are allocation
//! artefacts too: the model numbers them in creation order, a reader in document order. They are
//! paired the same way as part ids: each index site extends a one-to-one map
//! `authoritative index ↔ reopened index`, and paired records must be equal by value (kind,
//! author, date, move name). A split or merge is a difference. Every reopened record must be
//! referenced; an authoritative record no site references (left behind when a tracked insertion
//! is itself deleted) carries no content and is not written, so it is not a difference.
//!
//! The walk destructures every struct and enum on the way to these sites without `..`, so a field
//! or variant added to the model later fails to compile here until it is classified.
//!
//! # Fail-closed
//!
//! Only `Ok(Equivalence::Equivalent)` means equivalent. A reference to a missing part, a part kind
//! the site can't hold, an authoritative part no root reaches, or an exceeded limit is an `Err`
//! and never equivalence. The walk is iterative (explicit work lists), counts its steps and bounds
//! its nesting depth ([`RoundTripLimits`]); its own memory is linear in the documents' sizes.

use std::collections::BTreeMap;

use crate::para::Run;
use crate::para::{GroupChild, NoteKind};
use crate::props::{CellProps, CharProps, NumChange, ParaProps, PropChange, RowProps, TableProps};
use crate::section::HeaderSet;
use crate::{Block, Blocks, Cell, Comment, Document, InlineObject, NoteSeparators, Paragraph, Part, PartKind, Row, SectionProps, Table, TextBody};

/// Which input a finding is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Authoritative,
    Reopened,
}

/// A structure the comparison can't prove equivalent or not. Never means "equivalent".
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoundTripError {
    #[error("{side:?} document references missing part {id}")]
    DanglingReference { side: Side, id: u32 },
    #[error("{side:?} part {id} is {found:?} where {expected:?} is referenced")]
    UnexpectedPartKind { side: Side, id: u32, expected: PartKind, found: PartKind },
    #[error("authoritative part {id} is not reachable from any stable root")]
    UnreachablePart { id: u32 },
    #[error("{side:?} document references missing revision {index}")]
    DanglingRevision { side: Side, index: u32 },
    #[error("round-trip comparison limit exceeded")]
    LimitExceeded,
    #[error("round-trip comparison could not allocate")]
    AllocationFailed,
}

/// The first semantic difference found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Difference {
    /// A field compared by value differs (`what` names it).
    Field(&'static str),
    /// One side has a reference (or an optional story) where the other has none.
    ReferencePresence(&'static str),
    /// Paired parts have different kinds.
    PartKind { authoritative: u32, reopened: u32 },
    /// One authoritative part corresponds to two reopened parts (a shared part was duplicated).
    Split { authoritative: u32 },
    /// Two authoritative parts correspond to one reopened part (distinct parts were merged).
    Merge { reopened: u32 },
    /// A reopened part no reference in the authoritative document corresponds to.
    ExtraPart { reopened: u32 },
    /// Two revision index sites of one document correspond to different revisions of the other.
    RevisionMapping { authoritative: u32, reopened: u32 },
    /// A reopened revision record no site references.
    ExtraRevision { reopened: u32 },
}

/// The verified one-to-one correspondence `authoritative part id → reopened part id`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PartMapping(BTreeMap<u32, u32>);

impl PartMapping {
    pub fn get(&self, authoritative: u32) -> Option<u32> {
        self.0.get(&authoritative).copied()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.0.iter().map(|(a, b)| (*a, *b))
    }
}

/// Result of a completed comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Equivalence {
    Equivalent(PartMapping),
    Different(Difference),
}

/// Bounds on one comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundTripLimits {
    /// Most blocks, objects, sections and parts visited (both documents together count once per
    /// pair).
    pub max_steps: u64,
    /// Deepest nesting of tables, groups, or section property-change chains.
    pub max_depth: u32,
}

impl Default for RoundTripLimits {
    fn default() -> Self {
        RoundTripLimits { max_steps: 50_000_000, max_depth: 512 }
    }
}

/// `Ok(true)` only when `reopened` is `authoritative` up to a permutation of part ids; see the
/// module docs. `Ok(false)` is a semantic difference, `Err` a structure that can't be proven.
pub fn roundtrip_equivalent(authoritative: &Document, reopened: &Document) -> Result<bool, RoundTripError> {
    Ok(matches!(compare_roundtrip(authoritative, reopened, &RoundTripLimits::default())?, Equivalence::Equivalent(_)))
}

/// [`roundtrip_equivalent`] with explicit limits, reporting the mapping or the first difference.
pub fn compare_roundtrip(authoritative: &Document, reopened: &Document, limits: &RoundTripLimits) -> Result<Equivalence, RoundTripError> {
    let mut cmp = Cmp {
        a: authoritative,
        b: reopened,
        fwd: BTreeMap::new(),
        bwd: BTreeMap::new(),
        rfwd: BTreeMap::new(),
        rbwd: BTreeMap::new(),
        work: Vec::new(),
        objects: Vec::new(),
        steps: 0,
        limits: *limits,
    };
    match cmp.document() {
        Ok(()) => Ok(Equivalence::Equivalent(PartMapping(cmp.fwd))),
        Err(Stop::Different(d)) => Ok(Equivalence::Different(d)),
        Err(Stop::Error(e)) => Err(e),
    }
}

enum Stop {
    Different(Difference),
    Error(RoundTripError),
}

type Step = Result<(), Stop>;

fn diff(d: Difference) -> Step {
    Err(Stop::Different(d))
}

fn fail(e: RoundTripError) -> Step {
    Err(Stop::Error(e))
}

fn check(same: bool, what: &'static str) -> Step {
    if same { Ok(()) } else { diff(Difference::Field(what)) }
}

/// Compare the listed value fields of two `$ty`s. The pattern names every field of `$ty` (value
/// fields, reference-bearing fields handled by the caller, and ignored ones) without `..`, so a
/// new field is a compile error here until it is classified.
macro_rules! same_fields {
    ($ty:ident { $($f:ident),* $(,)? } refs { $($g:ident),* $(,)? } $(ignore { $($h:ident),* $(,)? })? ; $a:expr, $b:expr) => {{
        let (a, b) = ($a, $b);
        let $ty { $($f: _,)* $($g: _,)* $($($h: _,)*)? } = a;
        true $(&& a.$f == b.$f)*
    }};
}

enum Work<'d> {
    Blocks(&'d Blocks, &'d Blocks, u32),
    Part(u32, u32),
}

struct Cmp<'d> {
    a: &'d Document,
    b: &'d Document,
    fwd: BTreeMap<u32, u32>,
    bwd: BTreeMap<u32, u32>,
    rfwd: BTreeMap<u32, u32>,
    rbwd: BTreeMap<u32, u32>,
    work: Vec<Work<'d>>,
    objects: Vec<(&'d InlineObject, &'d InlineObject, u32)>,
    steps: u64,
    limits: RoundTripLimits,
}

impl<'d> Cmp<'d> {
    fn tick(&mut self) -> Step {
        self.steps = self.steps.saturating_add(1);
        if self.steps > self.limits.max_steps { fail(RoundTripError::LimitExceeded) } else { Ok(()) }
    }

    fn deeper(&self, depth: u32) -> Result<u32, Stop> {
        let d = depth.saturating_add(1);
        if d > self.limits.max_depth { Err(Stop::Error(RoundTripError::LimitExceeded)) } else { Ok(d) }
    }

    fn push(&mut self, w: Work<'d>) -> Step {
        if self.work.try_reserve(1).is_err() {
            return fail(RoundTripError::AllocationFailed);
        }
        self.work.push(w);
        Ok(())
    }

    fn document(&mut self) -> Step {
        let (a, b) = (self.a, self.b);
        let Document {
            body: _,
            last_section: _,
            parts: _,
            comments: _,
            styles,
            numbering,
            revisions: _,
            settings,
            core,
            sources,
            custom_props,
            footnote_separators: _,
            endnote_separators: _,
            media,
            passthrough,
        } = a;
        check(*styles == b.styles, "styles")?;
        check(*numbering == b.numbering, "numbering")?;
        check(*settings == b.settings, "settings")?;
        check(*core == b.core, "core")?;
        check(*sources == b.sources, "sources")?;
        check(*custom_props == b.custom_props, "custom_props")?;
        check(*media == b.media, "media")?;
        check(*passthrough == b.passthrough, "passthrough")?;
        check(a.parts.len() == b.parts.len(), "parts.len")?;
        self.comments()?;
        self.section(&a.last_section, &b.last_section)?;
        self.separators(&a.footnote_separators, &b.footnote_separators)?;
        self.separators(&a.endnote_separators, &b.endnote_separators)?;
        self.push(Work::Blocks(&a.body, &b.body, 0))?;
        while let Some(w) = self.work.pop() {
            match w {
                Work::Blocks(x, y, depth) => self.blocks(x, y, depth)?,
                Work::Part(x, y) => self.part(x, y)?,
            }
        }
        if let Some(id) = a.parts.keys().find(|id| !self.fwd.contains_key(id)) {
            return fail(RoundTripError::UnreachablePart { id: *id });
        }
        if let Some(id) = b.parts.keys().find(|id| !self.bwd.contains_key(id)) {
            return diff(Difference::ExtraPart { reopened: *id });
        }
        if let Some(i) = (0..b.revisions.len()).find_map(|i| u32::try_from(i).ok().filter(|i| !self.rbwd.contains_key(i))) {
            return diff(Difference::ExtraRevision { reopened: i });
        }
        check(u32::try_from(b.revisions.len()).is_ok(), "revision count")
    }

    /// Pair revision index `x` (authoritative) with `y` (reopened): equal records, one-to-one.
    fn rev(&mut self, x: u32, y: u32) -> Step {
        self.tick()?;
        let Some(ra) = self.a.revisions.get(x as usize) else {
            return fail(RoundTripError::DanglingRevision { side: Side::Authoritative, index: x });
        };
        let Some(rb) = self.b.revisions.get(y as usize) else { return fail(RoundTripError::DanglingRevision { side: Side::Reopened, index: y }) };
        check(ra == rb, "revision")?;
        match (self.rfwd.get(&x).copied(), self.rbwd.get(&y).copied()) {
            (Some(m), Some(n)) if m == y && n == x => Ok(()),
            (None, None) => {
                self.rfwd.insert(x, y);
                self.rbwd.insert(y, x);
                Ok(())
            }
            _ => diff(Difference::RevisionMapping { authoritative: x, reopened: y }),
        }
    }

    fn opt_rev(&mut self, x: Option<u32>, y: Option<u32>, what: &'static str) -> Step {
        match (x, y) {
            (None, None) => Ok(()),
            (Some(x), Some(y)) => self.rev(x, y),
            _ => diff(Difference::ReferencePresence(what)),
        }
    }

    /// A tracked property change: presence, its revision, and its `old` value (compared by value,
    /// with any revision index of its own compared literally — the model keeps none there).
    fn change<T: PartialEq>(&mut self, x: &Option<Box<PropChange<T>>>, y: &Option<Box<PropChange<T>>>, what: &'static str) -> Step {
        match (x, y) {
            (None, None) => Ok(()),
            (Some(p), Some(q)) => {
                let PropChange { rev, old } = &**p;
                check(*old == q.old, what)?;
                self.rev(*rev, q.rev)
            }
            _ => diff(Difference::ReferencePresence(what)),
        }
    }

    /// Character properties up to revision indices, which are paired.
    fn char_props(&mut self, c: &CharProps, d: &CharProps) -> Step {
        if c.ins.is_none() && c.del.is_none() && c.fmt_change.is_none() && d.ins.is_none() && d.del.is_none() && d.fmt_change.is_none() {
            return check(c == d, "character properties");
        }
        let strip = |c: &CharProps| CharProps { ins: None, del: None, fmt_change: None, ..c.clone() };
        check(strip(c) == strip(d), "character properties")?;
        self.opt_rev(c.ins, d.ins, "tracked insertion")?;
        self.opt_rev(c.del, d.del, "tracked deletion")?;
        self.change(&c.fmt_change, &d.fmt_change, "tracked formatting")
    }

    fn para_props(&mut self, p: &ParaProps, q: &ParaProps) -> Step {
        if p.fmt_change.is_none() && p.num_change.is_none() && q.fmt_change.is_none() && q.num_change.is_none() {
            return check(p == q, "paragraph properties");
        }
        let strip = |p: &ParaProps| ParaProps { fmt_change: None, num_change: None, ..p.clone() };
        check(strip(p) == strip(q), "paragraph properties")?;
        self.change(&p.fmt_change, &q.fmt_change, "tracked paragraph formatting")?;
        match (&p.num_change, &q.num_change) {
            (None, None) => Ok(()),
            (Some(m), Some(n)) => {
                let NumChange { rev, original } = &**m;
                check(*original == n.original, "tracked numbering")?;
                self.rev(*rev, n.rev)
            }
            _ => diff(Difference::ReferencePresence("tracked numbering")),
        }
    }

    /// Pair a reference site holding `x` (authoritative) with the one holding `y` (reopened).
    fn pair(&mut self, expected: PartKind, x: u32, y: u32) -> Step {
        self.tick()?;
        let Some(pa) = self.a.parts.get(&x) else { return fail(RoundTripError::DanglingReference { side: Side::Authoritative, id: x }) };
        let Some(pb) = self.b.parts.get(&y) else { return fail(RoundTripError::DanglingReference { side: Side::Reopened, id: y }) };
        if pa.kind != expected {
            return fail(RoundTripError::UnexpectedPartKind { side: Side::Authoritative, id: x, expected, found: pa.kind });
        }
        if pb.kind != pa.kind {
            return diff(Difference::PartKind { authoritative: x, reopened: y });
        }
        match (self.fwd.get(&x).copied(), self.bwd.get(&y).copied()) {
            (Some(m), Some(n)) if m == y && n == x => Ok(()),
            (Some(_), _) => diff(Difference::Split { authoritative: x }),
            (_, Some(_)) => diff(Difference::Merge { reopened: y }),
            (None, None) => {
                self.fwd.insert(x, y);
                self.bwd.insert(y, x);
                self.push(Work::Part(x, y))
            }
        }
    }

    fn optional_ref(&mut self, expected: PartKind, x: &Option<u32>, y: &Option<u32>, what: &'static str) -> Step {
        match (x, y) {
            (None, None) => Ok(()),
            (Some(x), Some(y)) => self.pair(expected, *x, *y),
            _ => diff(Difference::ReferencePresence(what)),
        }
    }

    fn comments(&mut self) -> Step {
        let (a, b) = (self.a, self.b);
        check(a.comments.len() == b.comments.len(), "comments")?;
        for ((ia, ca), (ib, cb)) in a.comments.iter().zip(&b.comments) {
            self.tick()?;
            check(ia == ib, "comment id")?;
            check(same_fields!(Comment { author, initials, date, parent, resolved } refs { part }; ca, cb), "comment")?;
            self.pair(PartKind::Comment, ca.part, cb.part)?;
        }
        Ok(())
    }

    fn header_set(&mut self, x: &HeaderSet, y: &HeaderSet, kind: PartKind) -> Step {
        let HeaderSet { default, first, even } = x;
        self.optional_ref(kind, default, &y.default, "header/footer default")?;
        self.optional_ref(kind, first, &y.first, "header/footer first")?;
        self.optional_ref(kind, even, &y.even, "header/footer even")
    }

    /// A section and its chain of tracked property changes (iterative).
    fn section(&mut self, x: &'d SectionProps, y: &'d SectionProps) -> Step {
        let (mut x, mut y, mut depth) = (x, y, 0);
        loop {
            self.tick()?;
            check(
                same_fields!(SectionProps {
                    start, page_w, page_h, landscape, margin_top, margin_bottom, margin_left, margin_right, header, footer, gutter,
                    columns, title_page, page_num_start, page_num_format, line_numbers, valign, page_borders, rtl, text_direction,
                    doc_grid, footnote_pr, endnote_pr,
                } refs { headers, footers, fmt_change }; x, y),
                "section",
            )?;
            self.header_set(&x.headers, &y.headers, PartKind::Header)?;
            self.header_set(&x.footers, &y.footers, PartKind::Footer)?;
            match (&x.fmt_change, &y.fmt_change) {
                (None, None) => return Ok(()),
                (Some(p), Some(q)) => {
                    let PropChange { rev, old: _ } = &**p;
                    self.rev(*rev, q.rev)?;
                    depth = self.deeper(depth)?;
                    x = &p.old;
                    y = &q.old;
                }
                _ => return diff(Difference::ReferencePresence("section change")),
            }
        }
    }

    fn separators(&mut self, x: &'d NoteSeparators, y: &'d NoteSeparators) -> Step {
        let NoteSeparators { separator, continuation_separator, continuation_notice } = x;
        for (p, q) in [(separator, &y.separator), (continuation_separator, &y.continuation_separator), (continuation_notice, &y.continuation_notice)]
        {
            match (p, q) {
                (None, None) => {}
                (Some(p), Some(q)) => self.push(Work::Blocks(p, q, 0))?,
                _ => return diff(Difference::ReferencePresence("note separator")),
            }
        }
        Ok(())
    }

    fn part(&mut self, x: u32, y: u32) -> Step {
        self.tick()?;
        let (Some(pa), Some(pb)) = (self.a.parts.get(&x), self.b.parts.get(&y)) else {
            // `pair` checked both; a vanished part is still never equivalence.
            return fail(RoundTripError::DanglingReference { side: Side::Authoritative, id: x });
        };
        check(same_fields!(Part { kind } refs { blocks, body }; pa, pb), "part kind")?;
        check(same_fields!(TextBody { vert, anchor, inset, auto_fit } refs { next }; &pa.body, &pb.body), "text box body")?;
        self.optional_ref(PartKind::TextBox, &pa.body.next, &pb.body.next, "text box link")?;
        self.push(Work::Blocks(&pa.blocks, &pb.blocks, 0))
    }

    fn blocks(&mut self, x: &'d Blocks, y: &'d Blocks, depth: u32) -> Step {
        check(x.len() == y.len(), "block count")?;
        for (bx, by) in x.iter().zip(y) {
            self.tick()?;
            match (&**bx, &**by) {
                (Block::Para(p), Block::Para(q)) => self.para(p, q)?,
                (Block::Table(t), Block::Table(u)) => self.table(t, u, depth)?,
                _ => return diff(Difference::Field("block kind")),
            }
        }
        Ok(())
    }

    fn table(&mut self, t: &'d Table, u: &'d Table, depth: u32) -> Step {
        check(same_fields!(Table { grid, controls } refs { rows, props }; t, u), "table")?;
        check(TableProps { fmt_change: None, ..t.props.clone() } == TableProps { fmt_change: None, ..u.props.clone() }, "table")?;
        self.change(&t.props.fmt_change, &u.props.fmt_change, "tracked table formatting")?;
        check(t.rows.len() == u.rows.len(), "table rows")?;
        let inner = self.deeper(depth)?;
        for (r, s) in t.rows.iter().zip(&u.rows) {
            self.tick()?;
            check(same_fields!(Row { controls } refs { cells, props }; r, s), "table row")?;
            check(RowProps { fmt_change: None, ..r.props.clone() } == RowProps { fmt_change: None, ..s.props.clone() }, "table row")?;
            self.change(&r.props.fmt_change, &s.props.fmt_change, "tracked row formatting")?;
            check(r.cells.len() == s.cells.len(), "table cells")?;
            for (c, d) in r.cells.iter().zip(&s.cells) {
                check(same_fields!(Cell { controls } refs { blocks, props }; c, d), "table cell")?;
                check(CellProps { fmt_change: None, ..c.props.clone() } == CellProps { fmt_change: None, ..d.props.clone() }, "table cell")?;
                self.change(&c.props.fmt_change, &d.props.fmt_change, "tracked cell formatting")?;
                self.push(Work::Blocks(&c.blocks, &d.blocks, inner))?;
            }
        }
        Ok(())
    }

    fn para(&mut self, p: &'d Paragraph, q: &'d Paragraph) -> Step {
        // `rev` is a cache counter, not part of `Paragraph`'s equality either.
        check(same_fields!(Paragraph { text } refs { objects, section, runs, props, mark } ignore { rev }; p, q), "paragraph")?;
        check(p.runs.len() == q.runs.len(), "paragraph")?;
        for (r, t) in p.runs.iter().zip(&q.runs) {
            self.tick()?;
            let Run { len, props } = r;
            check(*len == t.len, "paragraph")?;
            self.char_props(props, &t.props)?;
        }
        self.para_props(&p.props, &q.props)?;
        self.char_props(&p.mark, &q.mark)?;
        check(p.objects.len() == q.objects.len(), "object count")?;
        for (o, r) in p.objects.iter().zip(&q.objects) {
            self.object(o, r)?;
        }
        match (&p.section, &q.section) {
            (None, None) => Ok(()),
            (Some(s), Some(t)) => self.section(s, t),
            _ => diff(Difference::ReferencePresence("paragraph section")),
        }
    }

    /// An inline object and, for groups, its members (iterative).
    fn object(&mut self, o: &'d InlineObject, r: &'d InlineObject) -> Step {
        self.objects.clear();
        if self.objects.try_reserve(1).is_err() {
            return fail(RoundTripError::AllocationFailed);
        }
        self.objects.push((o, r, 0));
        while let Some((o, r, depth)) = self.objects.pop() {
            self.tick()?;
            match o {
                InlineObject::Shape { kind, w, h, fill, stroke, stroke_width, float, story, freeform, effects, extra } => {
                    let InlineObject::Shape {
                        kind: kind2,
                        w: w2,
                        h: h2,
                        fill: fill2,
                        stroke: stroke2,
                        stroke_width: sw2,
                        float: float2,
                        story: story2,
                        freeform: ff2,
                        effects: fx2,
                        extra: ex2,
                    } = r
                    else {
                        return diff(Difference::Field("inline object kind"));
                    };
                    check(
                        kind == kind2
                            && w == w2
                            && h == h2
                            && fill == fill2
                            && stroke == stroke2
                            && stroke_width == sw2
                            && float == float2
                            && freeform == ff2
                            && effects == fx2
                            && extra == ex2,
                        "shape",
                    )?;
                    self.optional_ref(PartKind::TextBox, story, story2, "text box story")?;
                }
                InlineObject::Group { w, h, float, ch_w, ch_h, children, canvas } => {
                    let InlineObject::Group { w: w2, h: h2, float: float2, ch_w: cw2, ch_h: ch2, children: children2, canvas: canvas2 } = r else {
                        return diff(Difference::Field("inline object kind"));
                    };
                    check(w == w2 && h == h2 && float == float2 && ch_w == cw2 && ch_h == ch2 && canvas == canvas2, "group")?;
                    check(children.len() == children2.len(), "group members")?;
                    let inner = self.deeper(depth)?;
                    if self.objects.try_reserve(children.len()).is_err() {
                        return fail(RoundTripError::AllocationFailed);
                    }
                    for (c, d) in children.iter().zip(children2) {
                        let GroupChild { x, y, obj } = c;
                        check(*x == d.x && *y == d.y, "group member position")?;
                        self.objects.push((obj, &d.obj, inner));
                    }
                }
                InlineObject::NoteRef { kind, id, custom } => {
                    let InlineObject::NoteRef { kind: kind2, id: id2, custom: custom2 } = r else {
                        return diff(Difference::Field("inline object kind"));
                    };
                    check(kind == kind2 && custom == custom2, "note reference")?;
                    let part = match kind {
                        NoteKind::Footnote => PartKind::Footnote,
                        NoteKind::Endnote => PartKind::Endnote,
                    };
                    self.pair(part, *id, *id2)?;
                }
                // No part ids in these: compared by value. Fields are spelled out so a new
                // (possibly reference-bearing) field is a compile error here.
                InlineObject::Image { media: _, w: _, h: _, alt: _, float: _, crop: _, ole: _ }
                | InlineObject::Graphic { w: _, h: _, alt: _, float: _, graphic: _ }
                | InlineObject::Field { instr: _, result: _, locked: _, code: _ }
                | InlineObject::FieldStart { instr: _, locked: _ }
                | InlineObject::FieldEnd
                | InlineObject::ControlStart { control: _ }
                | InlineObject::ControlEnd
                | InlineObject::BookmarkStart { name: _ }
                | InlineObject::BookmarkEnd { name: _ }
                | InlineObject::CommentStart { id: _ }
                | InlineObject::CommentEnd { id: _ }
                | InlineObject::PermStart { id: _, group: _, editor: _ }
                | InlineObject::PermEnd { id: _ }
                | InlineObject::Equation { linear: _, display: _, math: _ }
                | InlineObject::Ruby { base: _, ruby: _, align: _, size: _, raise: _, base_size: _, lang: _, props: _ }
                | InlineObject::Opaque { format: _, xml: _, text: _ } => check(o == r, "inline object")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "roundtrip_tests.rs"]
mod tests;
