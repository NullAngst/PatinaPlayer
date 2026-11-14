//! The smart playlist editor's form: rules as rows of field, condition and
//! value, matched all or any, with a limit and an order. It reads and
//! writes the same query text the playlists are stored as, so a form and
//! the text box are two views of one thing.
//!
//! Not every query fits the rows: nested groups, a mix of "and" and "or",
//! or the leave-out box. [`Form::from_text`] gives None for those, and the
//! editor shows the text instead.

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

/// The whole form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// Match any rule rather than all of them.
    pub any: bool,
    pub rules: Vec<Rule>,
    pub limit: Option<Limit>,
    /// An index into [`ORDERS`].
    pub order: usize,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            any: false,
            rules: vec![Rule::new(Field::Artist)],
            limit: None,
            order: 0,
        }
    }
}

impl Form {
    /// The query text. Rows still missing a value are left out, so a half
    /// filled form still says what it matches so far.
    pub fn to_text(&self) -> String {
        let join = if self.any { " or " } else { " and " };
        let mut t = self
            .rules
            .iter()
            .filter_map(Rule::text)
            .collect::<Vec<_>>()
            .join(join);
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
        let rules = items
            .into_iter()
            .map(rule_from)
            .collect::<Option<Vec<_>>>()?;
        let order = match q.sort {
            None => 0,
            Some((SortKey::Random, _)) => 1,
            Some(s) => ORDERS.iter().position(|o| o.1 == Some(s))?,
        };
        Some(Form {
            any,
            rules: if rules.is_empty() {
                Form::default().rules
            } else {
                rules
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
            rules: vec![
                rule(Field::Rating, RuleOp::Greater, "3"),
                rule(Field::Genre, RuleOp::Is, "blues rock"),
                rule(Field::Artist, RuleOp::NotContains, "live"),
                range,
                age,
                rule(Field::Bpm, RuleOp::Less, "130"),
                rule(Field::Composer, RuleOp::Starts, "J.S."),
                rule(Field::Lossless, RuleOp::Yes, ""),
                rule(Field::Added, RuleOp::After, "2024-01-31"),
            ],
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
            rules: vec![
                rule(Field::Artist, RuleOp::Is, "Nirvana"),
                rule(Field::Artist, RuleOp::Is, "Mudhoney"),
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
        assert_eq!(f.rules, vec![rule(Field::Rating, RuleOp::Greater, "3")]);
        assert_eq!(ORDERS[f.order].0, "highest rating");
        let f = Form::from_text("rating >= 2 and rating <= 3", "").unwrap();
        assert_eq!(f.rules[1], rule(Field::Rating, RuleOp::Less, "4"));
        let f = Form::from_text("added < 30d sort by added", "").unwrap();
        assert_eq!(f.rules[0].op, RuleOp::InLast);
        assert_eq!(f.rules[0].value, "30");
        let f = Form::from_text("", "").unwrap();
        assert_eq!(f.rules.len(), 1);
    }

    #[test]
    fn queries_that_dont_fit_stay_text() {
        for (q, x) in [
            ("rating 5 or rating 4 and plays 0", ""),
            ("(artist ~ a or artist ~ b) and rating 5", ""),
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
            rules: vec![
                rule(Field::Artist, RuleOp::Contains, ""),
                rule(Field::Year, RuleOp::Is, "1991"),
            ],
            limit: Some(Limit::Songs(25)),
            order: 1,
        };
        assert_eq!(f.to_text(), "year = 1991 sort by random limit 25");
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
