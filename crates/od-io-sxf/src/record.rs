//! Splits an SFC `DATA;` section into `/*SXF ... SXF*/` feature records, and
//! each record's parameter list into plain strings.
//!
//! SFC quotes every parameter, but with two different delimiters depending on
//! whether the value is a genuine string or not: `'value'` for a number (or a
//! `(a,b,c)` list of them), `\'value\'` for real text (a layer name, a piece
//! of drawing text). This distinction only matters while scanning — once a
//! parameter's text is extracted, a caller that already knows "the third
//! parameter of `line_feature` is a coordinate" can just parse it, so both
//! delimiter kinds collapse into one `Vec<String>` here rather than a typed
//! enum the rest of the crate would have to keep unwrapping.

/// One `#<id> = <name>(<params>)` record found inside a `/*SXF ... SXF*/`
/// block. `raw` is the whole record's own text, kept so an unrecognised
/// `name` can be preserved byte-for-byte (`docs/CLAUDE.md` rule 2) rather
/// than silently dropped.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Record<'a> {
    pub id: u64,
    pub name: &'a str,
    pub params: Vec<String>,
    pub raw: &'a str,
}

/// Finds every `/*SXF ... SXF*/` block in `text` and parses the one record
/// each contains. A block that does not parse as `#id = name(...)` is
/// skipped rather than failing the whole read — the same "never fail the
/// whole file for one bad entity" rule `od-io-dxf` follows.
pub(crate) fn scan_records(text: &str) -> Vec<Record<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*SXF") {
        let after_open = &rest[start + "/*SXF".len()..];
        let Some(end) = after_open.find("SXF*/") else {
            break;
        };
        let body = after_open[..end].trim();
        if let Some(record) = parse_record(body) {
            out.push(record);
        }
        rest = &after_open[end + "SXF*/".len()..];
    }
    out
}

fn parse_record(block: &str) -> Option<Record<'_>> {
    let rest = block.trim().strip_prefix('#')?;
    let digits_end = rest.find(|c: char| !c.is_ascii_digit())?;
    let id: u64 = rest[..digits_end].parse().ok()?;
    let rest = rest[digits_end..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let paren = rest.find('(')?;
    let name = rest[..paren].trim();
    let after = &rest[paren + 1..];
    let close = after.trim_end().rfind(')')?;
    let params = parse_params(&after[..close]);
    Some(Record {
        id,
        name,
        params,
        raw: block,
    })
}

/// Splits a record's parameter list on top-level commas, stripping each
/// parameter's own delimiter (`'...'` or `\'...\'`) — the same rules
/// `SfcHelper`'s tokenizer uses (cross-checked against a real SFC example in
/// NILIM's CAD製図基準 guidance, `ks0403012.pdf` §6.1): a lone `\` inside a
/// `\'...\'` string is literal unless it is immediately followed by `'`, in
/// which case that pair closes the string.
fn parse_params(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ',' => {
                chars.next();
            }
            _ if c.is_whitespace() => {
                chars.next();
            }
            '\'' => {
                chars.next();
                let mut buf = String::new();
                for ch in chars.by_ref() {
                    if ch == '\'' {
                        break;
                    }
                    buf.push(ch);
                }
                out.push(buf);
            }
            '\\' => {
                chars.next();
                if chars.peek() != Some(&'\'') {
                    continue;
                }
                chars.next();
                let mut buf = String::new();
                loop {
                    match chars.next() {
                        None => break,
                        Some('\\') if chars.peek() == Some(&'\'') => {
                            chars.next();
                            break;
                        }
                        Some(ch) => buf.push(ch),
                    }
                }
                out.push(buf);
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

/// Parses a `'(v1,v2,...)'` list parameter's already-unwrapped inner text
/// (the caller has already stripped the outer parameter quotes; this only
/// strips the `(` `)`).
pub(crate) fn parse_list(s: &str) -> Vec<f64> {
    s.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .filter(|p| !p.trim().is_empty())
        .filter_map(|p| p.trim().parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_line_feature_record_parses() {
        // Verbatim from NILIM's ks0403012.pdf §6.1, SXF(SFC)形式の場合.
        let text = "/*SXF\n#40 = line_feature('1','8','1','1','102.718015','46.660696','185.557783','149.775931')\nSXF*/";
        let records = scan_records(text);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.id, 40);
        assert_eq!(r.name, "line_feature");
        assert_eq!(
            r.params,
            vec![
                "1",
                "8",
                "1",
                "1",
                "102.718015",
                "46.660696",
                "185.557783",
                "149.775931"
            ]
        );
    }

    #[test]
    fn a_string_parameter_uses_the_backslash_quote_delimiter() {
        let text = "/*SXF\n#10 = layer_feature(\\'D-BGD\\','1')\nSXF*/";
        let records = scan_records(text);
        assert_eq!(records[0].params, vec!["D-BGD", "1"]);
    }

    #[test]
    fn a_list_parameter_round_trips_through_parse_list() {
        assert_eq!(parse_list("(6,1.5,0.25,1.5)"), vec![6.0, 1.5, 0.25, 1.5]);
    }

    #[test]
    fn an_unrecognised_block_is_skipped_not_fatal() {
        let text =
            "/*SXF\nnot a record at all\nSXF*/\n/*SXF\n#10 = layer_feature(\\'A\\','1')\nSXF*/";
        let records = scan_records(text);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "layer_feature");
    }
}
