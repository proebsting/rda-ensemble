//! What the run actually resolved to.
//!
//! Several options are shorthand: `--cycle` stands for three dataset names,
//! `--plan-type` stands for a district count, `--elections all` stands for
//! however many elections the file holds. A run that silently expanded those
//! would leave the user unable to say what they got, which matters more here
//! than in most tools -- an ensemble is evidence, and evidence whose inputs
//! are unclear is not worth much.
//!
//! So every resolved value is printed before the work starts, and anything
//! the command line did not say in so many words is annotated with where it
//! came from.

use std::fmt::Write as _;

/// One resolved value, and where it came from when that was not the command
/// line.
struct Row {
    name: &'static str,
    value: String,
    from: Option<String>,
}

#[derive(Default)]
pub struct Resolved {
    rows: Vec<Row>,
    sections: Vec<(&'static str, usize)>,
}

impl Resolved {
    pub fn section(&mut self, title: &'static str) -> &mut Self {
        self.sections.push((title, self.rows.len()));
        self
    }

    /// A value the command line gave.
    pub fn given(&mut self, name: &'static str, value: impl ToString) -> &mut Self {
        self.rows.push(Row { name, value: value.to_string(), from: None });
        self
    }

    /// A value the run worked out, and what it worked it out from.
    pub fn derived(
        &mut self,
        name: &'static str,
        value: impl ToString,
        from: impl ToString,
    ) -> &mut Self {
        self.rows.push(Row {
            name,
            value: value.to_string(),
            from: Some(from.to_string()),
        });
        self
    }

    /// A continuation line under the row just added, for a value too long
    /// to sit on one.
    pub fn note(&mut self, text: impl ToString) -> &mut Self {
        self.rows.push(Row { name: "", value: text.to_string(), from: None });
        self
    }

    pub fn render(&self) -> String {
        let width = self.rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
        // Capped: one long value -- a wrapped list of twenty elections --
        // should not push every other annotation off to the right.
        let value_width = self
            .rows
            .iter()
            .filter(|r| r.from.is_some())
            .map(|r| r.value.len())
            .max()
            .unwrap_or(0)
            .min(26);

        let mut out = String::new();
        for (i, row) in self.rows.iter().enumerate() {
            for (title, at) in &self.sections {
                if *at == i {
                    let _ = writeln!(out, "\n{title}");
                }
            }
            match &row.from {
                None => {
                    let _ = writeln!(out, "  {:<width$}  {}", row.name, row.value);
                }
                Some(from) => {
                    let _ = writeln!(
                        out,
                        "  {:<width$}  {:<value_width$}   {from}",
                        row.name, row.value
                    );
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Resolved;

    #[test]
    fn derived_values_carry_their_source_and_given_ones_do_not() {
        let mut s = Resolved::default();
        s.section("Input")
            .given("state", "NC")
            .derived("districts", 14, "statutory, NC congress")
            .given("rng seed", 1);
        let out = s.render();

        assert!(out.contains("\nInput\n"), "sections are printed: {out}");
        // Column widths are cosmetic; what matters is what each row says.
        let words = |needle: &str| -> Vec<String> {
            out.lines()
                .find(|l| l.contains(needle))
                .expect("the row")
                .split_whitespace()
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            words("districts"),
            ["districts", "14", "statutory,", "NC", "congress"],
            "a derived value names its source"
        );
        // A value the user typed needs no explanation of where it came from.
        assert_eq!(words("rng seed"), ["rng", "seed", "1"], "given values stand alone");
    }
}
