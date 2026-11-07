//! Line facets: kinds of lines the code lens can hide.
//!
//! A facet names a set of lines. Two sorts exist, and they hide differently:
//!
//! - **Tags** say *which lines* a facet covers — test code, generated files,
//!   benchmarks and examples. The collector files every line under the
//!   [`TagSet`] it carries, so the code layer holds, per language, one
//!   [`CodeNum`] per tag set
//!   ([`TagBuckets`]); hiding a tag drops every bucket that carries it, from
//!   every column.
//! - **Kinds** say *which column* a facet drops from the `lines` total —
//!   comments and blanks. Their own columns keep showing them.
//!
//! Hiding is view state ([`Hidden`]): the cached layer holds every bucket, so
//! a toggle re-reads it and never recomputes it.

use super::node::CodeNum;

/// One hideable kind of line. Exhaustive on purpose, like `Lens`: adding a
/// facet turns every site that must handle it into a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facet {
    Test,
    Generated,
    BenchExample,
    Comment,
    Blank,
}

impl Facet {
    /// Every facet, in display order.
    pub const ALL: [Facet; 5] = [
        Facet::Test,
        Facet::Generated,
        Facet::BenchExample,
        Facet::Comment,
        Facet::Blank,
    ];

    /// The name used in the header chips, help, and `--hide`.
    pub fn label(self) -> &'static str {
        match self {
            Facet::Test => "test",
            Facet::Generated => "generated",
            Facet::BenchExample => "bench",
            Facet::Comment => "comment",
            Facet::Blank => "blank",
        }
    }

    /// The key that toggles it.
    pub fn key(self) -> char {
        match self {
            Facet::Test => 't',
            Facet::Generated => 'n',
            Facet::BenchExample => 'o',
            Facet::Comment => 'c',
            Facet::Blank => 'b',
        }
    }

    /// Whether the collector can produce this facet's lines yet. A facet that
    /// is not live has no key, no chip, and is rejected by `--hide`.
    pub fn is_live(self) -> bool {
        match self {
            Facet::Test | Facet::Generated | Facet::Comment | Facet::Blank => true,
            Facet::BenchExample => false,
        }
    }

    /// The facets the user can toggle, in display order.
    pub fn live() -> impl Iterator<Item = Facet> {
        Self::ALL.into_iter().filter(|facet| facet.is_live())
    }

    /// The facet with this `--hide` name, if any.
    pub fn from_label(label: &str) -> Option<Facet> {
        Self::ALL.into_iter().find(|facet| facet.label() == label)
    }

    /// The tag this facet hides, for tag facets.
    fn tag(self) -> TagSet {
        match self {
            Facet::Test => TagSet::TEST,
            Facet::Generated => TagSet::GENERATED,
            Facet::BenchExample => TagSet::BENCH,
            Facet::Comment | Facet::Blank => TagSet::EMPTY,
        }
    }

    fn bit(self) -> u8 {
        match self {
            Facet::Test => 1,
            Facet::Generated => 1 << 1,
            Facet::BenchExample => 1 << 2,
            Facet::Comment => 1 << 3,
            Facet::Blank => 1 << 4,
        }
    }
}

/// The tags a line carries: a bit set over the tag facets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TagSet(u8);

impl TagSet {
    pub const EMPTY: TagSet = TagSet(0);
    pub const TEST: TagSet = TagSet(1);
    pub const GENERATED: TagSet = TagSet(1 << 1);
    pub const BENCH: TagSet = TagSet(1 << 2);

    pub fn union(self, other: TagSet) -> TagSet {
        TagSet(self.0 | other.0)
    }

    pub fn intersects(self, other: TagSet) -> bool {
        self.0 & other.0 != 0
    }
}

/// The facets currently hidden. Empty by default: every line counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hidden(u8);

impl Hidden {
    pub fn contains(self, facet: Facet) -> bool {
        self.0 & facet.bit() != 0
    }

    pub fn insert(&mut self, facet: Facet) {
        self.0 |= facet.bit();
    }

    pub fn toggle(&mut self, facet: Facet) {
        self.0 ^= facet.bit();
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The hidden facets, in display order.
    pub fn iter(self) -> impl Iterator<Item = Facet> {
        Facet::ALL.into_iter().filter(move |&f| self.contains(f))
    }

    /// The tags whose lines are hidden.
    pub fn tags(self) -> TagSet {
        self.iter()
            .fold(TagSet::EMPTY, |acc, facet| acc.union(facet.tag()))
    }

    /// `num` with the hidden kinds zeroed — what counts toward `lines`.
    pub fn counted(self, num: CodeNum) -> CodeNum {
        CodeNum {
            code: num.code,
            comments: if self.contains(Facet::Comment) {
                0
            } else {
                num.comments
            },
            blanks: if self.contains(Facet::Blank) {
                0
            } else {
                num.blanks
            },
        }
    }
}

/// Line counts split by the tags the lines carry: a sparse map from
/// [`TagSet`] to [`CodeNum`], kept sorted, with at most one entry per tag set
/// (eight). Most files have exactly one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagBuckets(Vec<(TagSet, CodeNum)>);

impl TagBuckets {
    /// A single bucket.
    pub fn of(tags: TagSet, num: CodeNum) -> Self {
        let mut buckets = Self::default();
        buckets.add(tags, num);
        buckets
    }

    /// Add `num` to the bucket for `tags`.
    pub fn add(&mut self, tags: TagSet, num: CodeNum) {
        match self.0.binary_search_by_key(&tags, |&(t, _)| t) {
            Ok(i) => self.0[i].1 += num,
            Err(i) => self.0.insert(i, (tags, num)),
        }
    }

    /// File every bucket under its tags plus `tags`, merging buckets that
    /// meet.
    pub fn retag(&mut self, tags: TagSet) {
        for (bucket, num) in std::mem::take(&mut self.0) {
            self.add(bucket.union(tags), num);
        }
    }

    /// Add every bucket of `other`.
    pub fn merge(&mut self, other: &TagBuckets) {
        for &(tags, num) in &other.0 {
            self.add(tags, num);
        }
    }

    /// Every line, whatever its tags.
    pub fn total(&self) -> CodeNum {
        self.visible(TagSet::EMPTY)
    }

    /// The lines carrying none of the `hidden` tags.
    pub fn visible(&self, hidden: TagSet) -> CodeNum {
        self.0
            .iter()
            .filter(|(tags, _)| !tags.intersects(hidden))
            .fold(CodeNum::default(), |mut acc, &(_, num)| {
                acc += num;
                acc
            })
    }

    /// The buckets, in tag-set order.
    pub fn iter(&self) -> impl Iterator<Item = (TagSet, CodeNum)> + '_ {
        self.0.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(code: usize, comments: usize, blanks: usize) -> CodeNum {
        CodeNum {
            code,
            comments,
            blanks,
        }
    }

    #[test]
    fn facets_have_distinct_keys_bits_and_labels() {
        for (i, a) in Facet::ALL.iter().enumerate() {
            for b in &Facet::ALL[i + 1..] {
                assert_ne!(a.key(), b.key());
                assert_ne!(a.bit(), b.bit());
                assert_ne!(a.label(), b.label());
            }
            assert_eq!(Facet::from_label(a.label()), Some(*a));
        }
        assert_eq!(Facet::from_label("nope"), None);
    }

    #[test]
    fn hidden_toggles_and_maps_to_tags() {
        let mut hidden = Hidden::default();
        assert!(hidden.is_empty());
        hidden.toggle(Facet::Test);
        hidden.insert(Facet::Blank);
        assert!(hidden.contains(Facet::Test) && hidden.contains(Facet::Blank));
        assert_eq!(hidden.tags(), TagSet::TEST);
        assert_eq!(
            hidden.iter().collect::<Vec<_>>(),
            [Facet::Test, Facet::Blank]
        );
        hidden.toggle(Facet::Test);
        assert!(!hidden.contains(Facet::Test));
        assert_eq!(hidden.tags(), TagSet::EMPTY);
    }

    #[test]
    fn counted_zeroes_only_the_hidden_kinds() {
        let n = num(5, 3, 2);
        let mut hidden = Hidden::default();
        assert_eq!(hidden.counted(n), n);
        hidden.insert(Facet::Comment);
        assert_eq!(hidden.counted(n), num(5, 0, 2));
        hidden.insert(Facet::Blank);
        assert_eq!(hidden.counted(n), num(5, 0, 0));
        hidden.insert(Facet::Test); // a tag facet does not touch the kinds
        assert_eq!(hidden.counted(n), num(5, 0, 0));
    }

    /// Hiding a tag removes each line once, however many hidden tags it
    /// carries, and the buckets always sum to the total.
    #[test]
    fn buckets_filter_by_tag_without_double_counting() {
        let mut buckets = TagBuckets::of(TagSet::EMPTY, num(10, 0, 0));
        buckets.add(TagSet::TEST, num(4, 0, 0));
        buckets.add(TagSet::TEST.union(TagSet::BENCH), num(2, 0, 0));
        buckets.add(TagSet::BENCH, num(1, 0, 0));
        buckets.add(TagSet::TEST, num(1, 1, 1));

        assert_eq!(buckets.total(), num(18, 1, 1));
        assert_eq!(buckets.visible(TagSet::TEST), num(11, 0, 0));
        assert_eq!(buckets.visible(TagSet::BENCH), num(15, 1, 1));
        assert_eq!(
            buckets.visible(TagSet::TEST.union(TagSet::BENCH)),
            num(10, 0, 0)
        );
        assert_eq!(buckets.iter().count(), 4, "one entry per tag set");
    }

    #[test]
    fn merge_adds_bucket_by_bucket() {
        let mut a = TagBuckets::of(TagSet::EMPTY, num(1, 0, 0));
        let b = TagBuckets::of(TagSet::TEST, num(2, 0, 0));
        a.merge(&b);
        a.merge(&b);
        assert_eq!(a.visible(TagSet::EMPTY), num(5, 0, 0));
        assert_eq!(a.visible(TagSet::TEST), num(1, 0, 0));
    }
}
