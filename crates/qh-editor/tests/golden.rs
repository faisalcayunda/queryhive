//! G1: golden files. For every `tests/fixtures/corpus/*.sql`, the colour runs of one full paint
//! (`.classes`) and the statements, folds and issues (`.outline`) are compared with the files
//! next to it. `QH_BLESS=1 cargo test -p qh-editor --test golden` writes them again; the diff is
//! a change of appearance and is reviewed as one.
//!
//! A file is read under the dialect its name starts with (`pg-`, `mysql-`, `trino-`; anything
//! else is generic).

mod common;

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use qh_editor::{Class, Document, FoldKind, IssueKind};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus")
}

fn class_name(code: u32) -> &'static str {
    [
        Class::Comment,
        Class::String,
        Class::QuotedIdentifier,
        Class::Number,
        Class::Keyword,
        Class::Literal,
        Class::Function,
        Class::Punctuation,
        Class::Parameter,
    ][code as usize - 1]
        .name()
}

fn render(name: &str, text: &str) -> (String, String) {
    let mut doc = Document::new(text, common::dialect_of(name)).unwrap();
    let paint = doc.paint(1, 0, u32::MAX, u32::MAX).unwrap();
    common::validate(&paint);
    assert!(!paint.more_in_window && !paint.dirty_elsewhere && !paint.inactive);
    let units: Vec<u16> = text.encode_utf16().collect();
    let slice = |start: u32, len: u32| {
        String::from_utf16_lossy(&units[start as usize..(start + len) as usize])
    };

    let mut classes = String::new();
    for run in paint.runs.chunks(3) {
        writeln!(
            classes,
            "{} {} {} {:?}",
            run[0],
            run[1],
            class_name(run[2]),
            slice(run[0], run[1])
        )
        .unwrap();
    }

    let outline = doc.outline(1).unwrap();
    let mut out = String::new();
    for pair in outline.statements.chunks(2) {
        writeln!(out, "statement {} {}", pair[0], pair[1]).unwrap();
    }
    for fold in &outline.folds {
        let kind = match fold.kind {
            FoldKind::Statement => "statement",
            FoldKind::Cte => "cte",
            FoldKind::Subquery => "subquery",
            FoldKind::Body => "body",
        };
        writeln!(
            out,
            "fold {kind} lines {}-{} header {} body {}..{} {:?}",
            fold.header_line,
            fold.last_line,
            fold.header,
            fold.body_start,
            fold.body_end,
            fold.summary
        )
        .unwrap();
    }
    for issue in &outline.issues {
        let kind = match issue.kind {
            IssueKind::UnclosedQuote => "unclosed-quote",
            IssueKind::UnclosedIdentifier => "unclosed-identifier",
            IssueKind::UnclosedComment => "unclosed-comment",
            IssueKind::UnclosedDollar => "unclosed-dollar",
            IssueKind::UnbalancedParen => "unbalanced-paren",
            IssueKind::SyntaxError => "syntax-error",
            IssueKind::MissingToken => "missing-token",
        };
        writeln!(out, "issue {kind} {} {}", issue.start, issue.len).unwrap();
    }
    (classes, out)
}

#[test]
fn every_corpus_file_paints_as_its_golden_files_say() {
    let bless = std::env::var_os("QH_BLESS").is_some();
    let mut names: Vec<String> = fs::read_dir(corpus())
        .unwrap()
        .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
        .filter(|name| name.ends_with(".sql"))
        .collect();
    names.sort();
    assert!(names.len() >= 15, "the corpus is missing: {names:?}");

    let mut wrong = Vec::new();
    for name in &names {
        let text = fs::read_to_string(corpus().join(name)).unwrap();
        let (classes, outline) = render(name, &text);
        for (extension, got) in [("classes", classes), ("outline", outline)] {
            let path = corpus().join(name.replace(".sql", &format!(".{extension}")));
            if bless {
                fs::write(&path, &got).unwrap();
            } else {
                match fs::read_to_string(&path) {
                    Ok(want) if want == got => {}
                    Ok(_) => wrong.push(format!("{} differs", path.display())),
                    Err(_) => wrong.push(format!(
                        "{} is missing (QH_BLESS=1 writes it)",
                        path.display()
                    )),
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
