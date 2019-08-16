use derive_more::{From, Into};
use shrinkwraprs::Shrinkwrap;
use indexmap::map::{self, IndexMap};
use std::borrow::Cow;
use std::iter::{self, FromIterator};
use std::prelude::v1::*;

pub mod parser;
pub mod writer;

#[derive(PartialEq, Eq, Debug, From, Into, Shrinkwrap)]
#[shrinkwrap(mutable)]
pub struct Dialog<'a>(pub IndexMap<&'a str, DialogEntry<'a>>);

impl<'a> FromIterator<DialogKey<'a>> for Dialog<'a> {
    fn from_iter<I: IntoIterator<Item = DialogKey<'a>>>(iter: I) -> Self {
        Dialog(iter.into_iter().map(Into::into).collect())
    }
}

impl<'a> FromIterator<(&'a str, DialogEntry<'a>)> for Dialog<'a> {
    fn from_iter<I: IntoIterator<Item = (&'a str, DialogEntry<'a>)>>(iter: I) -> Self {
        Dialog(iter.into_iter().collect())
    }
}

type KeyTuple<'a> = (&'a str, DialogEntry<'a>);
type KeyRefTuple<'a, 'b> = (&'b &'a str, &'b DialogEntry<'a>);
type TupleToKey<'a> = fn(KeyTuple<'a>) -> DialogKey<'a>;
type RefTupleToKey<'a, 'b> = fn(KeyRefTuple<'a, 'b>) -> DialogKey<'a>;

type DialogIntoIter<'a> = map::IntoIter<&'a str, DialogEntry<'a>>;
type DialogIter<'a, 'b> = map::Iter<'b, &'a str, DialogEntry<'a>>;

pub struct IntoIter<'a>(iter::Map<DialogIntoIter<'a>, TupleToKey<'a>>);

impl<'a> Iterator for IntoIter<'a> {
    type Item = DialogKey<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

pub struct Iter<'a, 'b>(iter::Map<DialogIter<'a, 'b>, RefTupleToKey<'a, 'b>>);

impl<'a> Iterator for Iter<'a, '_> {
    type Item = DialogKey<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl<'a> IntoIterator for Dialog<'a> {
    type Item = DialogKey<'a>;
    type IntoIter = IntoIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter(self.0.into_iter().map(From::from))
    }
}

impl<'a, 'b> IntoIterator for &'b Dialog<'a> {
    type Item = DialogKey<'a>;
    type IntoIter = Iter<'a, 'b>;

    fn into_iter(self) -> Self::IntoIter {
        Iter(self.0.iter().map(From::from))
    }
}

impl<'a> Dialog<'a> {
    pub fn new() -> Self {
        Dialog(IndexMap::new())
    }

    pub fn insert<'b: 'a>(&mut self, key: DialogKey<'b>) -> Option<DialogKey<'a>> {
        let DialogKey(name, entry) = key;
        self.0.insert(name, entry).map(|e| DialogKey(key.0, e))
    }

    pub fn iter(&self) -> <&Self as IntoIterator>::IntoIter {
        self.into_iter()
    }
}

impl Default for Dialog<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, From, Into)]
pub struct DialogKey<'a>(pub &'a str, pub DialogEntry<'a>);

impl<'a, 'b> From<KeyRefTuple<'a, 'b>> for DialogKey<'a> {
    fn from((&name, &entry): KeyRefTuple<'a, 'b>) -> Self {
        DialogKey(name, entry)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
pub struct DialogEntry<'a> {
    pub indented_str: &'a str,
    pub level: usize,
}

impl DialogEntry<'_> {
    pub fn unindent(&self) -> Cow<str> {
        let mut counter = 0;
        let trim = move |c| {
            if counter < self.level && (c == '\t' || c == ' ') {
                counter += 1;
                true
            } else {
                false
            }
        };

        if (self.level == 0 || self.indented_str.trim_start().lines().count() <= 1)
            && !self.indented_str.contains('#')
        {
            match self.indented_str.chars().nth(0) {
                Some('\r') => &self.indented_str[2..],
                Some('\n') => &self.indented_str[1..],
                _ => self.indented_str,
            }
            .trim_start_matches(trim)
            .into()
        } else {
            let mut string = String::with_capacity(self.indented_str.len());

            for line in self
                .indented_str
                .trim_start()
                .lines()
                .map(|s| s.trim_start_matches(trim))
            {
                if line.contains('#') {
                    let mut chars = line.chars().peekable();
                    while let Some(chr) = chars.next() {
                        if chr == '\\' && chars.peek() == Some(&'#') {
                            string.push('#');
                            let _ = chars.next();
                        } else {
                            string.push(chr);
                        }
                    }
                } else {
                    string.push_str(line);
                }
                string.push('\n');
            }
            string.into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unindented_entry() {
        let string = "ABC=\r\n123\r\n456";
        let slice = &string[4..];
        assert_eq!(slice, "\r\n123\r\n456", "sanity check");
        let entry = DialogEntry {
            indented_str: slice,
            level: 0,
        };
        assert_eq!(entry.unindent(), Cow::Borrowed("123\r\n456"));
    }
    #[test]
    fn short_entry() {
        let string = "ABC=\t123";
        let slice = &string[4..];
        assert_eq!(slice, "\t123", "sanity check");
        let entry = DialogEntry {
            indented_str: slice,
            level: 1,
        };
        assert_eq!(entry.unindent(), Cow::Borrowed("123"));
    }
    #[test]
    fn long_entry() {
        let string = "ABC=\n\t123\n\t456";
        let slice = &string[4..];
        assert_eq!(slice, "\n\t123\n\t456", "sanity check");
        let entry = DialogEntry {
            indented_str: slice,
            level: 1,
        };
        assert_eq!(entry.unindent(), Cow::Borrowed("123\n456\n").into_owned());
    }
}
