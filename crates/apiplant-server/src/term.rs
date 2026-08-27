//! What the command line looks like.
//!
//! Every `apiplant` subcommand prints through here, so that `build`, `seed`,
//! `studio` and the server's own start-up banner read as one program rather
//! than nine authors: the same indent, the same wordmark, the same box around
//! a set of links, the same green tick on the line that says it worked.
//!
//! Two rules hold the look together. Everything is indented two spaces, so a
//! block of output has a margin and the tick and the box line up. And every
//! escape comes from [`Style::detect`], which is empty when stdout is not a
//! terminal — piping any of this into a file gets plain text.

use std::io::IsTerminal;

/// The wordmark.
///
/// Lowercase, because the product is `apiplant` — it is the binary's name, the
/// crate's name and the way it is written everywhere else, and a banner
/// shouting APIPLANT at the top of every `run` was the one place that
/// disagreed. The letters are drawn in the same idiom the uppercase block was:
/// solid `█` strokes with the light box-drawing characters tracing the edge
/// they would cast one cell down and to the right, which is what gives it the
/// extruded look. Seven rows rather than six, because lowercase has an
/// ascender band above the `a` and the `p` hangs a row below the baseline.
///
/// The `a` carries a serif at its bottom-right corner — the stroke kicks one
/// cell past the stem at the baseline. Without it the bowl is symmetrical and
/// the letter reads as an `o`, which at this size is the difference between a
/// wordmark and a puzzle.
pub const WORDMARK: &str = r"                   ██╗          ██╗
                   ╚═╝          ██║                      ██╗
 ██████╗  ██████╗  ██╗ ██████╗  ██║  ██████╗  ███████╗ ██████╗
██╔══██║  ██╔══██╗ ██║ ██╔══██╗ ██║ ██╔══██║  ██╔══██║ ╚═██╔═╝
╚███████╗ ██████╔╝ ██║ ██████╔╝ ██║ ╚███████╗ ██║  ██║   ██║
 ╚══════╝ ██╔═══╝  ╚═╝ ██╔═══╝  ╚═╝  ╚══════╝ ╚═╝  ╚═╝   ╚═╝
          ╚═╝          ╚═╝";

/// The margin every line of output starts with.
const INDENT: &str = "  ";

/// ANSI escapes, or empty strings when stdout is not a terminal.
pub struct Style {
    pub bold: &'static str,
    pub red: &'static str,
    pub blue: &'static str,
    pub green: &'static str,
    pub yellow: &'static str,
    pub dim: &'static str,
    pub reset: &'static str,
}

impl Style {
    /// The escapes to use right now: real ones for a terminal, empty otherwise.
    pub fn detect() -> Self {
        if std::io::stdout().is_terminal() {
            Style {
                bold: "\x1b[1m",
                red: "\x1b[31m",
                blue: "\x1b[34m",
                green: "\x1b[32m",
                yellow: "\x1b[33m",
                dim: "\x1b[2m",
                reset: "\x1b[0m",
            }
        } else {
            Style {
                bold: "",
                red: "",
                blue: "",
                green: "",
                yellow: "",
                dim: "",
                reset: "",
            }
        }
    }
}

/// Print the wordmark, followed by a blank line.
///
/// Only the two commands that then sit there serving — `run` and `studio` —
/// print it. A one-shot command that finishes in a second gets a heading.
pub fn wordmark() {
    let s = Style::detect();
    println!();
    for line in WORDMARK.lines() {
        println!("{INDENT}{}{line}{}", s.blue, s.reset);
    }
}

/// The line that names what is happening: `build ./app`, or an app's name.
///
/// `subject` is the thing being acted on, dimmed so the verb reads first.
pub fn heading(what: &str, subject: Option<&str>) {
    let s = Style::detect();
    println!();
    match subject {
        Some(subject) => println!(
            "{INDENT}{b}{what}{r} {d}{subject}{r}",
            b = s.bold,
            d = s.dim,
            r = s.reset
        ),
        None => println!("{INDENT}{b}{what}{r}", b = s.bold, r = s.reset),
    }
    println!();
}

/// One line of a list under a heading — a file written, a resource loaded.
pub fn item(text: &str) {
    let s = Style::detect();
    println!("{INDENT}  {d}{text}{r}", d = s.dim, r = s.reset);
}

/// One line of a two-column list: a name, then what happened to it.
pub fn detail(name: &str, text: &str) {
    let s = Style::detect();
    println!("{INDENT}  {name:<24}{d}{text}{r}", d = s.dim, r = s.reset);
}

/// The last line: it worked, and here is what it did.
pub fn done(text: &str) {
    let s = Style::detect();
    println!();
    println!("{INDENT}{g}✓{r} {text}", g = s.green, r = s.reset);
    println!();
}

/// The last line when it did not work.
///
/// Goes to stderr, so a command whose stdout is being piped somewhere still
/// says why it produced nothing.
pub fn fail(text: &str) {
    let s = Style::detect();
    eprintln!();
    eprintln!("{INDENT}{r_}✗{r} {text}", r_ = s.red, r = s.reset);
}

/// Something the user should know but which did not stop the command.
pub fn note(text: &str) {
    let s = Style::detect();
    println!(
        "{INDENT}{y}!{r} {d}{text}{r}",
        y = s.yellow,
        d = s.dim,
        r = s.reset
    );
}

/// A "what to do next" block: a command per line, with what it is for.
pub fn next(steps: &[(String, &str)]) {
    let s = Style::detect();
    println!("{INDENT}{b}Next{r}", b = s.bold, r = s.reset);
    let width = steps
        .iter()
        .map(|(command, _)| command.chars().count())
        .max()
        .unwrap_or(0);
    for (command, purpose) in steps {
        let pad = width - command.chars().count();
        println!(
            "{INDENT}  {command}{}  {d}{purpose}{r}",
            " ".repeat(pad),
            d = s.dim,
            r = s.reset
        );
    }
    println!();
}

/// A box of labelled links, titled with the host they belong to.
///
/// This is the address book a long-running command leaves on screen: where the
/// API is, where the dashboard is, where the editor is. The box is as wide as
/// its widest row, title included, since the title sits on the top border.
pub fn links(title: &str, rows: &[(&str, String)]) {
    let s = Style::detect();
    let lines: Vec<String> = rows
        .iter()
        .map(|(label, url)| format!("{label:<7}{url}"))
        .collect();
    let inner = lines
        .iter()
        .map(|line| line.chars().count())
        .chain(std::iter::once(title.chars().count() + 3))
        .max()
        .unwrap_or(0)
        + 2;

    let title_fill = inner - title.chars().count() - 3;
    println!(
        "{INDENT}{d}┌─{r} {b}{title}{r} {d}{}┐{r}",
        "─".repeat(title_fill),
        d = s.dim,
        b = s.blue,
        r = s.reset
    );
    for (line, (label, url)) in lines.iter().zip(rows) {
        let pad = inner - line.chars().count() - 1;
        println!(
            "{INDENT}{d}│{r} {d}{label:<7}{r}{url}{}{d}│{r}",
            " ".repeat(pad),
            d = s.dim,
            r = s.reset
        );
    }
    println!(
        "{INDENT}{d}└{}┘{r}",
        "─".repeat(inner),
        d = s.dim,
        r = s.reset
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wordmark_fits_an_eighty_column_terminal() {
        // Lines are right-trimmed, so they differ in length; what matters is
        // that the widest still fits next to the two-space margin.
        let widths: Vec<usize> = WORDMARK.lines().map(|l| l.chars().count()).collect();
        assert_eq!(
            widths.len(),
            7,
            "ascender row, five body rows, descender row"
        );
        assert!(widths.iter().all(|w| w + INDENT.len() <= 80), "{widths:?}");
        // Drawn, not spelled: nothing here is a letter the terminal has to have
        // a glyph for beyond the block-drawing range.
        assert!(!WORDMARK.contains(char::is_alphabetic));
    }

    #[test]
    fn a_box_survives_a_title_wider_than_its_rows() {
        // The title sits on the border, so it has to be counted when the width
        // is chosen — otherwise `repeat` is handed a negative length.
        links("a.io", &[("API", "http://a.io".into())]);
        links(
            "a-very-long-domain-name.example.test",
            &[("API", "http://x".into())],
        );
    }

    #[test]
    fn styles_are_empty_when_output_is_not_a_terminal() {
        // Tests do not run on a tty, so this is the piped case.
        let s = Style::detect();
        assert_eq!(s.bold, "");
        assert_eq!(s.reset, "");
    }
}
