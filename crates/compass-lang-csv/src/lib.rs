//! `compass-lang-csv` — the CSV/TSV **catalog** extractor (ADR-0007).
//!
//! A data file has no imports and no calls; what a developer — or an assistant — needs from it
//! is *what the data is called*: is the customer column `Customer_Id`, `customerId` or `cust_no`,
//! and which of the forty files under `data/` has it? So the header's column names become
//! [`SymbolKind::Field`] symbols, and `find_symbol("customer")` answers both questions in one
//! call.
//!
//! Only the top of the file is ever read ([`Parsing::Head`]): the header costs the same for a
//! 2 GB export as for a 2 KB sample, and no row of data ever enters the map. Files are in the
//! `data` category, so they stay out of every dependency metric and are hidden in the visual
//! map until the viewer asks for them.

use compass_core::{FileCategory, LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, Parsing, RawImport,
    ResolutionContext, ResolvedImport,
};

/// How much of a file to read to find its header. A header of 200 long column names fits many
/// times over; a preamble of comment lines still leaves room.
const HEAD_BYTES: usize = 64 * 1024;

/// Past this many columns a "header" is a wide machine export (one column per sensor, per day…)
/// whose names nobody looks up one by one.
const MAX_COLUMNS: usize = 200;

/// Delimiters a `.csv` may use despite its name, in the order ties are broken.
const CSV_DELIMITERS: [u8; 4] = *b",;\t|";

/// The CSV/TSV extractor. Registered by the CLI composition root (ADR-0003).
pub struct CsvExtractor;

impl Extractor for CsvExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("csv")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["csv", "tsv"],
            shebangs: &[],
        }
    }

    fn category(&self) -> FileCategory {
        FileCategory::new("data")
    }

    fn parsing(&self) -> Parsing {
        Parsing::Head {
            max_bytes: HEAD_BYTES,
        }
    }

    fn extract_head(&self, head: &[u8]) -> Extraction {
        Extraction {
            symbols: header_columns(head),
            ..Extraction::default()
        }
    }

    /// A data file references nothing.
    fn resolve(
        &self,
        _imports: &[RawImport],
        _ctx: &dyn ResolutionContext,
        _config: &LangConfig,
    ) -> Vec<ResolvedImport> {
        Vec::new()
    }
}

/// The header's column names as symbols, or nothing if the file has no recognisable header.
fn header_columns(head: &[u8]) -> Vec<ExtractedSymbol> {
    // Excel writes a UTF-8 byte-order mark; it is not part of the first column's name.
    const BOM: &[u8] = b"\xEF\xBB\xBF";
    let (head, offset) = match head.strip_prefix(BOM) {
        Some(rest) => (rest, BOM.len()),
        None => (head, 0),
    };

    let Some((row, line)) = header_line(head) else {
        return Vec::new();
    };
    let cells = split_record(line, delimiter_of(line));
    if !looks_like_a_header(&cells) {
        return Vec::new();
    }

    let mut seen = std::collections::HashSet::new();
    cells
        .into_iter()
        .take(MAX_COLUMNS)
        .filter(|cell| !cell.name.is_empty() && seen.insert(cell.name.clone()))
        .map(|cell| ExtractedSymbol {
            name: cell.name,
            kind: SymbolKind::Field,
            span: Span {
                start_byte: offset + cell.start,
                end_byte: offset + cell.end,
                start_row: row,
                start_col: cell.start - line.start,
            },
        })
        .collect()
}

/// A line of the head, with where it starts in it.
#[derive(Clone, Copy)]
struct Line<'a> {
    bytes: &'a [u8],
    start: usize,
}

/// The first line that can be a header: not blank, not a `#` comment (a common preamble in
/// scientific exports). Only complete lines count — a head that ends mid-line is a header too
/// long to trust. Returns the line with its 0-based row.
fn header_line(head: &[u8]) -> Option<(usize, Line<'_>)> {
    let mut start = 0usize;
    for (row, raw) in head.split(|&b| b == b'\n').enumerate() {
        let complete = start + raw.len() < head.len() || head.len() < HEAD_BYTES;
        let bytes = raw.strip_suffix(b"\r").unwrap_or(raw);
        let blank = bytes.iter().all(u8::is_ascii_whitespace);
        if !blank && !bytes.starts_with(b"#") {
            return complete.then_some((row, Line { bytes, start }));
        }
        start += raw.len() + 1;
    }
    None
}

/// The delimiter is whichever candidate splits the header into the most cells outside quotes,
/// whatever the extension says: European "CSV" exports use `;`, database dumps `|`, and a
/// `.tsv` is simply the case where tab wins.
fn delimiter_of(line: Line<'_>) -> u8 {
    let count = |delimiter: u8| {
        let mut quoted = false;
        line.bytes
            .iter()
            .filter(|&&b| {
                if b == b'"' {
                    quoted = !quoted;
                }
                !quoted && b == delimiter
            })
            .count()
    };
    // `max_by_key` keeps the LAST maximum, so iterate in reverse to prefer the earlier candidate.
    CSV_DELIMITERS
        .iter()
        .rev()
        .copied()
        .max_by_key(|&d| count(d))
        .unwrap_or(b',')
}

/// One header cell: its cleaned name and its byte range in the head.
struct Cell {
    name: String,
    start: usize,
    end: usize,
}

/// Split one record on `delimiter`, honouring RFC 4180 quoting (`"a, b"`, `"say ""hi"""`).
fn split_record(line: Line<'_>, delimiter: u8) -> Vec<Cell> {
    let mut cells = Vec::new();
    let (mut cell_start, mut quoted) = (0usize, false);
    for i in 0..=line.bytes.len() {
        let at_end = i == line.bytes.len();
        if !at_end && line.bytes[i] == b'"' {
            quoted = !quoted;
        }
        if at_end || (!quoted && line.bytes[i] == delimiter) {
            let raw = String::from_utf8_lossy(&line.bytes[cell_start..i]);
            let trimmed = raw.trim();
            let name = match trimmed.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                Some(inner) => inner.replace("\"\"", "\"").trim().to_string(),
                None => trimmed.to_string(),
            };
            cells.push(Cell {
                name,
                start: line.start + cell_start,
                end: line.start + i,
            });
            cell_start = i + 1;
        }
    }
    cells
}

/// A first row of *names*, not of data. A file without a header starts with values, and values
/// are mostly numbers, dates or blanks; names are neither. One column can't be told either way
/// from a single line, so a lone non-numeric cell is accepted.
fn looks_like_a_header(cells: &[Cell]) -> bool {
    let named = cells.iter().filter(|c| is_name(&c.name)).count();
    named * 2 > cells.len()
}

fn is_name(cell: &str) -> bool {
    let has_letter = cell.chars().any(char::is_alphabetic);
    let numeric = cell.parse::<f64>().is_ok();
    has_letter && !numeric
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(csv: &str) -> Vec<String> {
        CsvExtractor
            .extract_head(csv.as_bytes())
            .symbols
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    #[test]
    fn header_columns_become_field_symbols_with_their_position() {
        let extraction =
            CsvExtractor.extract_head(b"Customer_Id,order total,Created At\n17,9.50,2026-01-04\n");
        let got: Vec<(&str, SymbolKind, usize, usize)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.span.start_row, s.span.start_col))
            .collect();
        assert_eq!(
            got,
            [
                ("Customer_Id", SymbolKind::Field, 0, 0),
                ("order total", SymbolKind::Field, 0, 12),
                ("Created At", SymbolKind::Field, 0, 24),
            ]
        );
        // No row of data is ever a symbol, and a data file references nothing.
        assert!(extraction.imports.is_empty() && extraction.calls.is_empty());
    }

    #[test]
    fn delimiters_are_sniffed_and_quotes_are_honoured() {
        assert_eq!(
            columns("id;naam;bedrag\n1;Jan;9,50\n"),
            ["id", "naam", "bedrag"]
        );
        assert_eq!(columns("id\tname\ttotal\n"), ["id", "name", "total"]);
        assert_eq!(columns("id|name|total\n"), ["id", "name", "total"]);
        // A delimiter inside quotes is part of the name; `""` is an escaped quote.
        assert_eq!(
            columns("\"Last, First\",\"say \"\"hi\"\"\",age\n"),
            ["Last, First", "say \"hi\"", "age"]
        );
    }

    #[test]
    fn a_bom_a_comment_preamble_and_crlf_are_tolerated() {
        assert_eq!(columns("\u{feff}id,name\r\n1,Ann\r\n"), ["id", "name"]);
        assert_eq!(
            columns("# exported 2026-01-04\n# units: mm\n\nstation,depth\n7,120\n"),
            ["station", "depth"]
        );
    }

    #[test]
    fn a_file_without_a_header_row_yields_no_symbols() {
        // Values, not names.
        assert!(columns("17,9.50,2026-01-04\n18,3.20,2026-01-05\n").is_empty());
        assert!(columns("").is_empty());
        assert!(columns("\n\n").is_empty());
        // A single text cell is accepted: one column can't be judged from one line.
        assert_eq!(columns("email\nann@example.com\n"), ["email"]);
    }

    #[test]
    fn blank_and_repeated_column_names_are_dropped() {
        assert_eq!(columns("id,,name,id,\n"), ["id", "name"]);
    }

    #[test]
    fn a_header_too_long_for_the_head_is_not_trusted() {
        // The head is a prefix: if it ends before the first newline, the "header" is cut off
        // mid-name and would produce a wrong symbol.
        let endless = "column_name,".repeat(HEAD_BYTES / 12 + 1);
        assert!(CsvExtractor
            .extract_head(&endless.as_bytes()[..HEAD_BYTES])
            .symbols
            .is_empty());
    }

    #[test]
    fn very_wide_headers_are_capped() {
        let wide: Vec<String> = (0..500).map(|i| format!("sensor_{i}")).collect();
        assert_eq!(columns(&format!("{}\n", wide.join(","))).len(), MAX_COLUMNS);
    }

    #[test]
    fn it_is_a_head_only_data_extractor() {
        assert!(!CsvExtractor.category().is_code_like());
        assert!(matches!(
            CsvExtractor.parsing(),
            Parsing::Head { max_bytes } if max_bytes == HEAD_BYTES
        ));
    }
}
