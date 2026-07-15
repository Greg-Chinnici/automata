use std::fmt;

/// A Life-like rule in B/S notation, stored as neighbor-count bitmasks.
///
/// Bit `n` of `birth` set means a dead cell with `n` live neighbors is born;
/// bit `n` of `survival` set means a live cell with `n` live neighbors
/// survives. Lookup is two shifts in the hot loop — no dispatch, no branching
/// on rule identity.
///
/// Serializes as its notation string (`"B3/S23"`) so saved files stay
/// readable and shareable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BsRule {
    birth: u16,
    survival: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseRuleError(String);

impl fmt::Display for ParseRuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid rule notation: {}", self.0)
    }
}

impl std::error::Error for ParseRuleError {}

fn parse_counts(digits: &str, source: &str) -> Result<u16, ParseRuleError> {
    let mut mask = 0u16;
    for ch in digits.chars() {
        match ch.to_digit(10) {
            Some(n) if n <= 8 => mask |= 1 << n,
            _ => return Err(ParseRuleError(source.to_string())),
        }
    }
    Ok(mask)
}

impl BsRule {
    /// Parse B/S notation. Accepts `B3/S23` (case-insensitive, either order)
    /// and the bare Golly-style `23/3` form, which reads survival/birth.
    pub fn parse(notation: &str) -> Result<Self, ParseRuleError> {
        let err = || ParseRuleError(notation.to_string());
        let (a, b) = notation.trim().split_once('/').ok_or_else(err)?;
        let (a, b) = (a.trim(), b.trim());

        fn prefixed(part: &str, tag: char) -> Option<&str> {
            part.strip_prefix(tag)
                .or_else(|| part.strip_prefix(tag.to_ascii_lowercase()))
        }

        let (birth, survival) = if let Some(bd) = prefixed(a, 'B') {
            (bd, prefixed(b, 'S').ok_or_else(err)?)
        } else if let Some(sd) = prefixed(a, 'S') {
            let bd = prefixed(b, 'B').ok_or_else(err)?;
            (bd, sd)
        } else {
            // Bare `survival/birth`, e.g. `23/3` for Conway's Life.
            (b, a)
        };

        Ok(Self {
            birth: parse_counts(birth, notation)?,
            survival: parse_counts(survival, notation)?,
        })
    }

    /// Next state for a cell with the given number of live neighbors.
    #[inline]
    pub fn next_state(&self, alive: bool, neighbors: u8) -> bool {
        let mask = if alive { self.survival } else { self.birth };
        (mask >> neighbors) & 1 == 1
    }

    /// Canonical `B…/S…` notation for this rule.
    pub fn notation(&self) -> String {
        let digits = |mask: u16| -> String {
            (0..=8)
                .filter(|n| (mask >> n) & 1 == 1)
                .map(|n| char::from(b'0' + n as u8))
                .collect()
        };
        format!("B{}/S{}", digits(self.birth), digits(self.survival))
    }
}

impl TryFrom<String> for BsRule {
    type Error = ParseRuleError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<BsRule> for String {
    fn from(rule: BsRule) -> String {
        rule.notation()
    }
}

pub struct NamedRule {
    pub name: &'static str,
    pub rule: BsRule,
}

/// Built-in library of well-known Life-like rules.
pub fn builtin_rules() -> Vec<NamedRule> {
    [
        ("Life", "B3/S23"),
        ("HighLife", "B36/S23"),
        ("Seeds", "B2/S"),
        ("Life w/o Death", "B3/S012345678"),
        ("Day & Night", "B3678/S34678"),
        ("Maze", "B3/S12345"),
        ("Mazectric", "B3/S1234"),
        ("Replicator", "B1357/S1357"),
        ("2x2", "B36/S125"),
        ("Diamoeba", "B35678/S5678"),
        ("Anneal", "B4678/S35678"),
    ]
    .into_iter()
    .map(|(name, notation)| NamedRule {
        name,
        rule: BsRule::parse(notation).expect("built-in rule notation is valid"),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_notation() {
        let life = BsRule::parse("B3/S23").unwrap();
        assert!(life.next_state(false, 3)); // birth on 3
        assert!(!life.next_state(false, 2));
        assert!(life.next_state(true, 2)); // survival on 2 or 3
        assert!(life.next_state(true, 3));
        assert!(!life.next_state(true, 4));
    }

    #[test]
    fn parse_is_case_insensitive_and_order_insensitive() {
        let canonical = BsRule::parse("B36/S23").unwrap();
        assert_eq!(BsRule::parse("b36/s23").unwrap(), canonical);
        assert_eq!(BsRule::parse("S23/B36").unwrap(), canonical);
        assert_eq!(BsRule::parse("s23/b36").unwrap(), canonical);
    }

    #[test]
    fn parses_bare_golly_survival_slash_birth() {
        assert_eq!(
            BsRule::parse("23/3").unwrap(),
            BsRule::parse("B3/S23").unwrap()
        );
    }

    #[test]
    fn parses_empty_survival() {
        let seeds = BsRule::parse("B2/S").unwrap();
        assert!(seeds.next_state(false, 2));
        assert!(!seeds.next_state(true, 2)); // every live cell dies
    }

    #[test]
    fn rejects_malformed_notation() {
        for bad in ["", "B3", "B3/S9", "B3/23x", "B3-S23", "Bx/S23"] {
            assert!(BsRule::parse(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    #[test]
    fn notation_round_trips() {
        for text in ["B3/S23", "B36/S23", "B2/S", "B3678/S34678", "B3/S012345678"] {
            let rule = BsRule::parse(text).unwrap();
            assert_eq!(rule.notation(), text);
            assert_eq!(BsRule::parse(&rule.notation()).unwrap(), rule);
        }
    }

    #[test]
    fn builtin_library_parses() {
        let rules = builtin_rules();
        assert!(rules.len() >= 10);
        assert_eq!(rules[0].name, "Life");
        assert_eq!(rules[0].rule.notation(), "B3/S23");
    }
}
