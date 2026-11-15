//! The smart playlist editor's form: rules as rows of field, condition and
//! value, matched all or any, with a limit and an order. It reads and
//! writes the same query text the playlists are stored as, so a form and
//! the text box are two views of one thing.
//!
//! A row can also be a group, as in iTunes: rules of its own, matched all
//! or any apart from the rows around it. That's a bracket in the text, so
//! "rating 5 and (genre is blues or genre is jazz)" is two rows, the
//! second a group of two. Groups go one level deep.
//!
//! What doesn't fit: groups inside groups, "not" around a group, or the
//! leave-out box. [`Form::from_text`] gives None for those, and the editor
//! shows the text instead.
//!
//! The rows are kept flat, as [`Line`]s in the order they show, so a row's
//! place on screen is its index here.

use super::{Expr, Field, Kind, Limit, Op, SortKey, age, date, parse_with, range};

/// The fields, in the order the field menu lists them, with their names.
pub const FIELDS: &[(Field, &str)] = &[
    (Field::Title, "Title"),
    (Field::Artist, "Artist"),
    (Field::AlbumArtist, "Album artist"),
    (Field::Album, "Album"),
    (Field::Genre, "Genre"),
    (Field::Composer, "Composer"),
    (Field::Grouping, "Grouping"),
    (Field::Category, "Category"),
    (Field::Comment, "Comment"),
    (Field::Year, "Year"),
    (Field::Bpm, "BPM"),
    (Field::Rating, "Rating"),
    (Field::AlbumRating, "Album rating"),
    (Field::Plays, "Plays"),
    (Field::Played, "Last played"),
    (Field::Added, "Date added"),
    (Field::Modified, "Date modified"),
    (Field::Length, "Time"),
    (Field::TrackNo, "Track number"),
    (Field::DiscNo, "Disc number"),
    (Field::Format, "Format"),
    (Field::Lossless, "Lossless"),
    (Field::Rate, "Sample rate"),
    (Field::Bits, "Bit depth"),
    (Field::Bitrate, "Bit rate"),
    (Field::Channels, "Channels"),
    (Field::Size, "Size (MB)"),
    (Field::Folder, "Folder"),
    (Field::Source, "Source"),
];

/// A row's condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOp {
    Contains,
    NotContains,
    Is,
    IsNot,
    Starts,
    Ends,
    Greater,
    Less,
    Range,
    InLast,
    NotInLast,
    After,
    Before,
    Never,
    Yes,
    No,
}

/// What the value part of a row looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Text,
    Number,
    /// 0 to 5 stars.
    Stars,
    /// Two numbers.
    Range,
    /// A number and a unit: days, weeks, months, years.
    Age,
    /// A date, 2024-01-31.
    Date,
    /// Nothing to type.
    None,
}

/// The units of an age, as the unit menu lists them.
pub const AGE_UNITS: &[(&str, &str)] = &[
    ("d", "days"),
    ("w", "weeks"),
    ("mo", "months"),
    ("y", "years"),
];

/// Limits, as the unit menu lists them.
pub const LIMIT_UNITS: &[&str] = &["songs", "minutes", "hours", "MB", "GB"];

/// Orders, as "Sorted by" lists them. The first is the album order a
/// playlist has with no sort at all.
pub const ORDERS: &[(&str, Option<(SortKey, bool)>)] = &[
    ("album", None),
    ("random", Some((SortKey::Random, false))),
    ("artist", Some((SortKey::Artist, false))),
    ("title", Some((SortKey::Title, false))),
    ("highest rating", Some((SortKey::Rating, true))),
    ("lowest rating", Some((SortKey::Rating, false))),
    ("most recently played", Some((SortKey::Played, true))),
    ("least recently played", Some((SortKey::Played, false))),
    ("most often played", Some((SortKey::Plays, true))),
    ("least often played", Some((SortKey::Plays, false))),
    ("most recently added", Some((SortKey::Added, true))),
    ("least recently added", Some((SortKey::Added, false))),
    ("newest", Some((SortKey::Year, true))),
    ("oldest", Some((SortKey::Year, false))),
    ("longest", Some((SortKey::Length, true))),
    ("shortest", Some((SortKey::Length, false))),
];

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub field: Field,
    pub op: RuleOp,
    pub value: String,
    /// The top of a range.
    pub value2: String,
    /// An age's unit, an index into [`AGE_UNITS`].
    pub unit: usize,
}

impl Rule {
    pub fn new(field: Field) -> Rule {
        Rule {
            field,
            op: ops(field)[0],
            value: String::new(),
            value2: String::new(),
            unit: 0,
        }
    }

    /// Change the field, keeping the condition and value when they still
    /// make sense for it.
    pub fn set_field(&mut self, field: Field) {
        let same_kind = field_kind(field) == field_kind(self.field);
        self.field = field;
        if !ops(field).contains(&self.op) {
            self.op = ops(field)[0];
        }
        if !same_kind {
            self.value.clear();
            self.value2.clear();
        }
    }

    pub fn value_kind(&self) -> ValueKind {
        match self.op {
            RuleOp::Never | RuleOp::Yes | RuleOp::No => ValueKind::None,
            RuleOp::Range => ValueKind::Range,
            RuleOp::InLast | RuleOp::NotInLast => ValueKind::Age,
            RuleOp::After | RuleOp::Before => ValueKind::Date,
            _ if matches!(self.field, Field::Rating | Field::AlbumRating) => ValueKind::Stars,
            _ if field_kind(self.field) == Kind::Number => ValueKind::Number,
            _ => ValueKind::Text,
        }
    }

    /// The clause, or None while it's missing its value.
    fn text(&self) -> Option<String> {
        let f = self.field.name();
        let v = self.value.trim();
        let kind = self.value_kind();
        if !matches!(kind, ValueKind::None | ValueKind::Stars) && v.is_empty() {
            return None;
        }
        let q = quote(v);
        Some(match self.op {
            RuleOp::Contains => format!("{f} contains {q}"),
            RuleOp::NotContains => format!("not {f} contains {q}"),
            RuleOp::Is if kind == ValueKind::Stars => format!("{f} = {}", stars(v)),
            RuleOp::IsNot if kind == ValueKind::Stars => format!("{f} != {}", stars(v)),
            RuleOp::Greater if kind == ValueKind::Stars => format!("{f} > {}", stars(v)),
            RuleOp::Less if kind == ValueKind::Stars => format!("{f} < {}", stars(v)),
            RuleOp::Is if kind == ValueKind::Number => format!("{f} = {q}"),
            RuleOp::IsNot if kind == ValueKind::Number => format!("{f} != {q}"),
            RuleOp::Is => format!("{f} is {q}"),
            RuleOp::IsNot => format!("{f} is not {q}"),
            RuleOp::Starts => format!("{f} starts with {q}"),
            RuleOp::Ends => format!("{f} ends with {q}"),
            RuleOp::Greater => format!("{f} > {q}"),
            RuleOp::Less => format!("{f} < {q}"),
            RuleOp::Range => {
                let hi = self.value2.trim();
                if hi.is_empty() {
                    return None;
                }
                format!("{f} {v}-{hi}")
            }
            RuleOp::InLast | RuleOp::NotInLast => {
                let unit = AGE_UNITS.get(self.unit).map_or("d", |u| u.0);
                let sym = if self.op == RuleOp::InLast { "<" } else { ">" };
                format!("{f} {sym} {v}{unit}")
            }
            RuleOp::After => format!("{f} > {v}"),
            RuleOp::Before => format!("{f} < {v}"),
            RuleOp::Never => "never played".to_string(),
            RuleOp::Yes => "lossless".to_string(),
            RuleOp::No => "lossy".to_string(),
        })
    }
}

fn stars(v: &str) -> u8 {
    v.trim().parse::<u8>().unwrap_or(0).min(5)
}

fn field_kind(f: Field) -> Kind {
    f.kind()
}

/// The conditions a field takes, in menu order.
pub fn ops(field: Field) -> &'static [RuleOp] {
    use RuleOp::*;
    match field.kind() {
        Kind::Text => &[Contains, NotContains, Is, IsNot, Starts, Ends],
        Kind::Genre => &[Is, IsNot, Contains, NotContains],
        Kind::Number => &[Is, IsNot, Greater, Less, Range],
        Kind::Time if field == Field::Played => &[InLast, NotInLast, After, Before, Never],
        Kind::Time => &[InLast, NotInLast, After, Before],
        Kind::Bool => &[Yes, No],
        Kind::Folder => &[Is, IsNot, Contains],
        Kind::Source => &[Is, IsNot],
    }
}

/// A condition's words in the menu.
pub fn op_label(field: Field, op: RuleOp) -> &'static str {
    match op {
        RuleOp::Contains => "contains",
        RuleOp::NotContains => "does not contain",
        RuleOp::Is if field == Field::Folder => "is in",
        RuleOp::IsNot if field == Field::Folder => "is not in",
        RuleOp::Is => "is",
        RuleOp::IsNot => "is not",
        RuleOp::Starts => "starts with",
        RuleOp::Ends => "ends with",
        RuleOp::Greater => "is greater than",
        RuleOp::Less => "is less than",
        RuleOp::Range => "is in the range",
        RuleOp::InLast => "is in the last",
        RuleOp::NotInLast => "is not in the last",
        RuleOp::After => "is after",
        RuleOp::Before => "is before",
        RuleOp::Never => "never",
        RuleOp::Yes => "is true",
        RuleOp::No => "is false",
    }
}

/// A value as the query writes it: bare when it can be, quoted otherwise.
fn quote(v: &str) -> String {
    let bare = !v.is_empty()
        && v.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | ':' | '-' | '_' | '/' | '+'))
        && !matches!(
            v.to_lowercase().as_str(),
            "and" | "or" | "not" | "sort" | "order" | "limit" | "first" | "never"
        );
    if bare {
        v.to_string()
    } else if !v.contains('"') {
        format!("\"{v}\"")
    } else {
        // The query has no escapes: a value with both kinds of quote loses
        // its double ones.
        format!("'{}'", v.replace('\'', ""))
    }
}

/// A row of the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A rule. `inner` puts it in the group above it.
    Rule { rule: Rule, inner: bool },
    /// The head of a group: its rules are the inner ones right under it.
    Group { any: bool },
}

impl Line {
    pub fn top(rule: Rule) -> Line {
        Line::Rule { rule, inner: false }
    }

    pub fn inner(rule: Rule) -> Line {
        Line::Rule { rule, inner: true }
    }

    pub fn rule(&self) -> Option<&Rule> {
        match self {
            Line::Rule { rule, .. } => Some(rule),
            Line::Group { .. } => None,
        }
    }

    pub fn is_inner(&self) -> bool {
        matches!(self, Line::Rule { inner: true, .. })
    }
}

/// The whole form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// Match any row rather than all of them.
    pub any: bool,
    pub lines: Vec<Line>,
    pub limit: Option<Limit>,
    /// An index into [`ORDERS`].
    pub order: usize,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            any: false,
            lines: vec![Line::top(Rule::new(Field::Artist))],
            limit: None,
            order: 0,
        }
    }
}

/// "and" or "or".
fn joiner(any: bool) -> &'static str {
    if any { " or " } else { " and " }
}

impl Form {
    pub fn rule(&self, i: usize) -> Option<&Rule> {
        self.lines.get(i).and_then(Line::rule)
    }

    pub fn rule_mut(&mut self, i: usize) -> Option<&mut Rule> {
        match self.lines.get_mut(i) {
            Some(Line::Rule { rule, .. }) => Some(rule),
            _ => None,
        }
    }

    /// Where the row at `i` ends: past a group's rules for a group's head,
    /// else the next row.
    fn end_of(&self, i: usize) -> usize {
        let mut end = i + 1;
        if matches!(self.lines.get(i), Some(Line::Group { .. })) {
            while self.lines.get(end).is_some_and(Line::is_inner) {
                end += 1;
            }
        }
        end.min(self.lines.len())
    }

    /// A new rule after row `i`, with the same field. Under a group's
    /// head it goes after the group; next to a rule in a group, in the
    /// group.
    pub fn add_rule(&mut self, i: usize) {
        let field = (0..=i.min(self.lines.len().saturating_sub(1)))
            .rev()
            .find_map(|j| self.rule(j))
            .map_or(Field::Artist, |r| r.field);
        let inner = self.lines.get(i).is_some_and(Line::is_inner);
        let at = self.end_of(i);
        self.lines.insert(
            at,
            Line::Rule {
                rule: Rule::new(field),
                inner,
            },
        );
    }

    /// A new group after row `i` (after a whole group, for its head),
    /// with one rule in it. It matches the other way round from the rows
    /// around it, since that's what a group is for.
    pub fn add_group(&mut self, i: usize) {
        if self.lines.get(i).is_some_and(Line::is_inner) {
            return;
        }
        let field = self.rule(i).map_or(Field::Artist, |r| r.field);
        let at = self.end_of(i);
        self.lines.insert(at, Line::Group { any: !self.any });
        self.lines.insert(at + 1, Line::inner(Rule::new(field)));
    }

    /// Take out row `i`: a group's head takes its rules with it, and a
    /// group left with no rules goes too. The last row left is cleared
    /// rather than taken out.
    pub fn remove(&mut self, i: usize) {
        if i >= self.lines.len() {
            return;
        }
        let end = self.end_of(i);
        let was_inner = self.lines[i].is_inner();
        self.lines.drain(i..end);
        if was_inner {
            // The head is the nearest one above; drop it if it's now bare.
            if let Some(h) = (0..i).rev().find(|j| !self.lines[*j].is_inner())
                && matches!(self.lines[h], Line::Group { .. })
                && !self.lines.get(h + 1).is_some_and(Line::is_inner)
            {
                self.lines.remove(h);
            }
        }
        if self.lines.is_empty() {
            self.lines = Form::default().lines;
        }
    }

    /// Set a group's all or any, by its head's row.
    pub fn set_group_any(&mut self, i: usize, any: bool) {
        if let Some(Line::Group { any: a }) = self.lines.get_mut(i) {
            *a = any;
        }
    }

    /// The query text. Rows still missing a value are left out, so a half
    /// filled form still says what it matches so far.
    pub fn to_text(&self) -> String {
        let mut parts = Vec::new();
        let mut i = 0;
        while i < self.lines.len() {
            match &self.lines[i] {
                Line::Rule { rule, .. } => {
                    if let Some(t) = rule.text() {
                        parts.push(t);
                    }
                    i += 1;
                }
                Line::Group { any } => {
                    let end = self.end_of(i);
                    let inner: Vec<String> = self.lines[i + 1..end]
                        .iter()
                        .filter_map(|l| l.rule().and_then(Rule::text))
                        .collect();
                    match inner.len() {
                        0 => {}
                        1 => parts.push(inner.into_iter().next().unwrap()),
                        _ => parts.push(format!("({})", inner.join(joiner(*any)))),
                    }
                    i = end;
                }
            }
        }
        let mut t = parts.join(joiner(self.any));
        if let Some((key, desc)) = ORDERS.get(self.order).and_then(|o| o.1) {
            let dir = if desc { "desc" } else { "asc" };
            let k = match key {
                SortKey::Album => "album",
                SortKey::Title => "title",
                SortKey::Artist => "artist",
                SortKey::Year => "year",
                SortKey::Added => "added",
                SortKey::Played => "played",
                SortKey::Plays => "plays",
                SortKey::Rating => "rating",
                SortKey::Length => "length",
                SortKey::Random => "random",
            };
            if !t.is_empty() {
                t.push(' ');
            }
            if key == SortKey::Random {
                t.push_str("sort by random");
            } else {
                t.push_str(&format!("sort by {k} {dir}"));
            }
        }
        if let Some(l) = self.limit {
            if !t.is_empty() {
                t.push(' ');
            }
            t.push_str(&format!("limit {}", l.text()));
        }
        t
    }

    /// Read stored rules into the form. None when they don't fit rows.
    pub fn from_text(include: &str, exclude: &str) -> Option<Form> {
        if !exclude.trim().is_empty() {
            return None;
        }
        let q = parse_with(include, "").ok()?;
        let (any, items): (bool, Vec<&Expr>) = match &q.expr {
            None => (false, Vec::new()),
            Some(Expr::And(v)) => (false, v.iter().collect()),
            Some(Expr::Or(v)) => (true, v.iter().collect()),
            Some(e) => (false, vec![e]),
        };
        let mut lines = Vec::new();
        for e in items {
            let (group_any, inner) = match e {
                Expr::And(v) => (false, v),
                Expr::Or(v) => (true, v),
                e => {
                    lines.push(Line::top(rule_from(e)?));
                    continue;
                }
            };
            lines.push(Line::Group { any: group_any });
            for x in inner {
                // A group in a group doesn't fit.
                lines.push(Line::inner(rule_from(x)?));
            }
        }
        let order = match q.sort {
            None => 0,
            Some((SortKey::Random, _)) => 1,
            Some(s) => ORDERS.iter().position(|o| o.1 == Some(s))?,
        };
        Some(Form {
            any,
            lines: if lines.is_empty() {
                Form::default().lines
            } else {
                lines
            },
            limit: q.limit,
            order,
        })
    }
}

fn rule_from(e: &Expr) -> Option<Rule> {
    let r = |field: Field, op: RuleOp, value: &str| Rule {
        field,
        op,
        value: value.to_string(),
        value2: String::new(),
        unit: 0,
    };
    match e {
        Expr::Never(Field::Played) => Some(r(Field::Played, RuleOp::Never, "")),
        Expr::Not(x) => match x.as_ref() {
            Expr::Cond {
                field,
                op: Op::Contains,
                value,
            } if matches!(field.kind(), Kind::Text | Kind::Genre) => {
                Some(r(*field, RuleOp::NotContains, value))
            }
            Expr::Cond {
                field,
                op: Op::Is,
                value,
            } if matches!(field.kind(), Kind::Text | Kind::Genre) => {
                Some(r(*field, RuleOp::IsNot, value))
            }
            _ => None,
        },
        Expr::Cond { field, op, value } => {
            let f = *field;
            let int = |v: &str| v.trim().parse::<i64>().ok();
            match (f.kind(), op) {
                (Kind::Text, Op::Contains) | (Kind::Genre, Op::Contains) => {
                    Some(r(f, RuleOp::Contains, value))
                }
                (Kind::Text | Kind::Genre | Kind::Folder | Kind::Source, Op::Is) => {
                    Some(r(f, RuleOp::Is, value))
                }
                (Kind::Text | Kind::Genre | Kind::Folder | Kind::Source, Op::IsNot) => {
                    Some(r(f, RuleOp::IsNot, value))
                }
                (Kind::Text, Op::Starts) => Some(r(f, RuleOp::Starts, value)),
                (Kind::Text, Op::Ends) => Some(r(f, RuleOp::Ends, value)),
                (Kind::Folder, Op::Contains) => Some(r(f, RuleOp::Contains, value)),
                (Kind::Number, Op::Is) => match range(value) {
                    Some((lo, hi)) => {
                        let mut x = r(f, RuleOp::Range, &lo.to_string());
                        x.value2 = hi.to_string();
                        Some(x)
                    }
                    None => Some(r(f, RuleOp::Is, value)),
                },
                (Kind::Number, Op::IsNot) if range(value).is_none() => {
                    Some(r(f, RuleOp::IsNot, value))
                }
                (Kind::Number, Op::Gt) => Some(r(f, RuleOp::Greater, value)),
                (Kind::Number, Op::Lt) => Some(r(f, RuleOp::Less, value)),
                // ">= 4" is "greater than 3", for whole numbers.
                (Kind::Number, Op::Ge) => {
                    int(value).map(|n| r(f, RuleOp::Greater, &(n - 1).to_string()))
                }
                (Kind::Number, Op::Le) => {
                    int(value).map(|n| r(f, RuleOp::Less, &(n + 1).to_string()))
                }
                (Kind::Time, _) => {
                    let older = matches!(op, Op::Gt | Op::Ge);
                    if age(value).is_some() {
                        let v = value.trim().to_lowercase();
                        let i = v
                            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                            .unwrap_or(v.len());
                        let (n, unit) = (&v[..i], v[i..].trim());
                        let unit = match unit {
                            "" | "d" | "day" | "days" => 0,
                            "w" | "wk" | "week" | "weeks" => 1,
                            "m" | "mo" | "mon" | "month" | "months" => 2,
                            "y" | "yr" | "year" | "years" => 3,
                            // Hours don't have a row of their own.
                            _ => return None,
                        };
                        let mut x = r(
                            f,
                            if older {
                                RuleOp::NotInLast
                            } else {
                                RuleOp::InLast
                            },
                            n,
                        );
                        x.unit = unit;
                        Some(x)
                    } else if date(value).is_some() {
                        Some(r(
                            f,
                            if older { RuleOp::After } else { RuleOp::Before },
                            value,
                        ))
                    } else {
                        None
                    }
                }
                (Kind::Bool, Op::Is | Op::IsNot) => {
                    let yes = super::truth(value)? != (*op == Op::IsNot);
                    Some(r(f, if yes { RuleOp::Yes } else { RuleOp::No }, ""))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::smart::parse;

    fn rule(field: Field, op: RuleOp, value: &str) -> Rule {
        Rule {
            field,
            op,
            value: value.into(),
            value2: String::new(),
            unit: 0,
        }
    }

    #[test]
    fn a_form_writes_a_query_that_parses_and_reads_back() {
        let mut range = rule(Field::Year, RuleOp::Range, "1965");
        range.value2 = "1975".into();
        let mut age = rule(Field::Played, RuleOp::NotInLast, "6");
        age.unit = 2;
        let f = Form {
            any: false,
            lines: [
                rule(Field::Rating, RuleOp::Greater, "3"),
                rule(Field::Genre, RuleOp::Is, "blues rock"),
                rule(Field::Artist, RuleOp::NotContains, "live"),
                range,
                age,
                rule(Field::Bpm, RuleOp::Less, "130"),
                rule(Field::Composer, RuleOp::Starts, "J.S."),
                rule(Field::Lossless, RuleOp::Yes, ""),
                rule(Field::Added, RuleOp::After, "2024-01-31"),
            ]
            .into_iter()
            .map(Line::top)
            .collect(),
            limit: Some(Limit::Minutes(90)),
            order: 1,
        };
        let text = f.to_text();
        parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(Form::from_text(&text, ""), Some(f), "{text}");
    }

    #[test]
    fn any_joins_with_or() {
        let f = Form {
            any: true,
            lines: vec![
                Line::top(rule(Field::Artist, RuleOp::Is, "Nirvana")),
                Line::top(rule(Field::Artist, RuleOp::Is, "Mudhoney")),
            ],
            limit: None,
            order: 0,
        };
        assert_eq!(f.to_text(), "artist is Nirvana or artist is Mudhoney");
        assert_eq!(Form::from_text(&f.to_text(), ""), Some(f));
    }

    #[test]
    fn stored_queries_that_fit_rows_open_as_rows() {
        let f = Form::from_text("rating >= 4 sort by rating", "").unwrap();
        assert_eq!(
            f.lines,
            vec![Line::top(rule(Field::Rating, RuleOp::Greater, "3"))]
        );
        assert_eq!(ORDERS[f.order].0, "highest rating");
        let f = Form::from_text("rating >= 2 and rating <= 3", "").unwrap();
        assert_eq!(f.rule(1), Some(&rule(Field::Rating, RuleOp::Less, "4")));
        let f = Form::from_text("added < 30d sort by added", "").unwrap();
        assert_eq!(f.rule(0).unwrap().op, RuleOp::InLast);
        assert_eq!(f.rule(0).unwrap().value, "30");
        let f = Form::from_text("", "").unwrap();
        assert_eq!(f.lines.len(), 1);
    }

    #[test]
    fn queries_that_dont_fit_stay_text() {
        for (q, x) in [
            ("rating 5 or (rating 4 and (plays 0 or plays 1))", ""),
            ("rating 5 and not (artist ~ a or artist ~ b)", ""),
            ("rating 5", "artist is x"),
            ("played < 12h", ""),
            ("sort by album desc", ""),
        ] {
            assert_eq!(Form::from_text(q, x), None, "{q}");
        }
    }

    #[test]
    fn half_filled_rows_are_left_out() {
        let f = Form {
            any: false,
            lines: vec![
                Line::top(rule(Field::Artist, RuleOp::Contains, "")),
                Line::top(rule(Field::Year, RuleOp::Is, "1991")),
            ],
            limit: Some(Limit::Songs(25)),
            order: 1,
        };
        assert_eq!(f.to_text(), "year = 1991 sort by random limit 25");
    }

    #[test]
    fn groups_mix_and_with_or_both_ways() {
        // "and" between rows, "or" in the group.
        let f = Form {
            any: false,
            lines: vec![
                Line::top(rule(Field::Rating, RuleOp::Greater, "3")),
                Line::Group { any: true },
                Line::inner(rule(Field::Genre, RuleOp::Is, "blues")),
                Line::inner(rule(Field::Genre, RuleOp::Is, "jazz")),
            ],
            limit: None,
            order: 0,
        };
        let text = f.to_text();
        assert_eq!(text, "rating > 3 and (genre is blues or genre is jazz)");
        parse(&text).unwrap();
        assert_eq!(Form::from_text(&text, ""), Some(f));
        // And the other way: groups of "and", either one will do.
        let text = "(artist is Nirvana and year < 1992) or (artist is Mudhoney and lossless)";
        let f = Form::from_text(text, "").unwrap();
        assert!(f.any);
        assert_eq!(f.lines.len(), 6);
        assert_eq!(f.lines[0], Line::Group { any: false });
        assert_eq!(f.lines[3], Line::Group { any: false });
        assert_eq!(Form::from_text(&f.to_text(), ""), Some(f));
        // What text mode said before groups: "a or b and c" reads as
        // "a or (b and c)", and now fits.
        let f = Form::from_text("rating 5 or rating 4 and plays 0", "").unwrap();
        assert!(f.any);
        assert_eq!(f.lines[1], Line::Group { any: false });
    }

    #[test]
    fn rows_and_groups_come_and_go() {
        let mut f = Form::default();
        f.add_group(0);
        assert_eq!(f.lines.len(), 3);
        assert_eq!(f.lines[1], Line::Group { any: true });
        assert!(f.lines[2].is_inner());
        // + next to a rule in the group adds to the group.
        f.add_rule(2);
        assert!(f.lines[3].is_inner());
        // + on the group's head adds after the group, at the top level.
        f.add_rule(1);
        assert_eq!(f.lines.len(), 5);
        assert!(!f.lines[4].is_inner());
        // No groups inside groups.
        f.add_group(2);
        assert_eq!(f.lines.len(), 5);
        // Taking out the group's rules one by one takes the group too.
        f.remove(2);
        f.remove(2);
        assert_eq!(f.lines.len(), 2);
        assert!(
            f.lines
                .iter()
                .all(|l| matches!(l, Line::Rule { inner: false, .. }))
        );
        // Taking out a head takes its rules.
        f.add_group(1);
        f.remove(2);
        assert_eq!(f.lines.len(), 2);
        f.remove(0);
        f.remove(0);
        assert_eq!(f, Form::default());
    }

    #[test]
    fn values_are_quoted_when_they_need_it() {
        assert_eq!(quote("Nirvana"), "Nirvana");
        assert_eq!(quote("The Band"), "\"The Band\"");
        assert_eq!(quote("or"), "\"or\"");
        assert_eq!(quote("say \"hi\""), "'say \"hi\"'");
        let r = rule(Field::Title, RuleOp::Is, "Rock & Roll");
        parse(&r.text().unwrap()).unwrap();
    }

    #[test]
    fn changing_field_keeps_what_still_fits() {
        let mut r = rule(Field::Artist, RuleOp::Starts, "Nir");
        r.set_field(Field::Album);
        assert_eq!((r.op, r.value.as_str()), (RuleOp::Starts, "Nir"));
        r.set_field(Field::Year);
        assert_eq!((r.op, r.value.as_str()), (RuleOp::Is, ""));
        assert_eq!(r.value_kind(), ValueKind::Number);
        r.set_field(Field::Rating);
        assert_eq!(r.value_kind(), ValueKind::Stars);
    }
}
