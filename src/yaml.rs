// Minimal, dependency-free YAML parser.
//
// This intentionally supports only the subset of YAML the plugin config needs:
//   - block mappings            key: value
//   - block sequences           - item
//   - nesting via indentation   (spaces only; tabs are rejected)
//   - scalars                   plain / 'single' / "double" quoted strings,
//                               booleans, null, ints, floats
//   - comments                  # ... to end of line (ignored unless quoted)
//
// It does NOT support flow collections ({}, []), anchors/aliases, multi-line
// block scalars (|, >), or multiple documents. Those are not used by the config
// schema, and keeping the parser small keeps the plugin dependency-free.
//
// Parsed values are represented as serde_json::Value, which covers the whole
// scalar/sequence/mapping subset exactly.

use serde_json::{Map, Value};

pub fn yaml_error(message: &str, line: Option<usize>) -> String {
    match line {
        Some(l) => format!("{} (line {})", message, l + 1),
        None => message.to_string(),
    }
}

#[derive(Clone, Debug)]
struct Node {
    indent: usize,
    content: String,
    line: usize,
}

// Strip a trailing `#` comment from a line, respecting quotes. A `#` only
// starts a comment when it is at the start of the content or preceded by
// whitespace — `a#b` is not a comment.
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev: Option<char> = None;
    for (i, ch) in line.char_indices() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                } else if ch == '#' && prev.is_none_or(|p| p.is_whitespace()) {
                    return &line[..i];
                }
            }
        }
        prev = Some(ch);
    }
    line
}

// Tokenize into significant lines: { indent, content, line }.
fn tokenize(text: &str) -> Result<Vec<Node>, String> {
    let mut nodes = Vec::new();
    for (line, raw) in text.split('\n').enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        // Only reject tabs used for indentation; a tab inside a value is fine.
        let leading = &raw[..raw.len() - raw.trim_start_matches([' ', '\t']).len()];
        if leading.contains('\t') {
            return Err(yaml_error("tabs are not allowed for indentation", Some(line)));
        }
        let no_comment = strip_comment(raw);
        let trimmed_end = no_comment.trim_end();
        if trimmed_end.trim() == "" {
            continue; // blank / comment-only line
        }
        if trimmed_end.trim() == "---" {
            continue; // tolerate a document marker
        }
        let indent = trimmed_end.len() - trimmed_end.trim_start_matches(' ').len();
        nodes.push(Node {
            indent,
            content: trimmed_end[indent..].to_string(),
            line,
        });
    }
    Ok(nodes)
}

fn unescape_double(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek() {
                Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('r') => {
                    out.push('\r');
                    chars.next();
                }
                Some('t') => {
                    out.push('\t');
                    chars.next();
                }
                Some(&c @ ('"' | '\\' | '/')) => {
                    out.push(c);
                    chars.next();
                }
                _ => out.push(ch), // unknown escape: keep the backslash literally
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn is_int_literal(s: &str) -> bool {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn is_float_literal(s: &str) -> bool {
    // ^[-+]?(\d+\.\d*|\.\d+|\d+)([eE][-+]?\d+)?$
    let s = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let mantissa_ok = match mantissa.find('.') {
        Some(i) => {
            let (int, frac) = (&mantissa[..i], &mantissa[i + 1..]);
            let digits = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
            (!int.is_empty() && digits(int) && digits(frac)) // \d+\.\d*
                || (int.is_empty() && !frac.is_empty() && digits(frac)) // \.\d+
        }
        None => !mantissa.is_empty() && mantissa.bytes().all(|b| b.is_ascii_digit()),
    };
    let exponent_ok = match exponent {
        Some(e) => {
            let e = e.strip_prefix(['-', '+']).unwrap_or(e);
            !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit())
        }
        None => true,
    };
    mantissa_ok && exponent_ok
}

fn number(n: f64) -> Value {
    serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null)
}

fn parse_scalar(text: &str, line: usize) -> Result<Value, String> {
    let s = text.trim();
    if s.is_empty() {
        return Ok(Value::Null);
    }
    if let Some(rest) = s.strip_prefix('"') {
        return match rest.strip_suffix('"') {
            Some(inner) if s.len() >= 2 => Ok(Value::String(unescape_double(inner))),
            _ => Err(yaml_error("unterminated double-quoted string", Some(line))),
        };
    }
    if let Some(rest) = s.strip_prefix('\'') {
        return match rest.strip_suffix('\'') {
            Some(inner) if s.len() >= 2 => Ok(Value::String(inner.replace("''", "'"))),
            _ => Err(yaml_error("unterminated single-quoted string", Some(line))),
        };
    }
    let lower = s.to_lowercase();
    if lower == "null" || s == "~" {
        return Ok(Value::Null);
    }
    if lower == "true" {
        return Ok(Value::Bool(true));
    }
    if lower == "false" {
        return Ok(Value::Bool(false));
    }
    if is_int_literal(s) {
        return Ok(match s.parse::<i64>() {
            Ok(n) => Value::from(n),
            Err(_) => number(s.parse::<f64>().unwrap_or(f64::NAN)),
        });
    }
    if is_float_literal(s) {
        return Ok(number(s.parse::<f64>().unwrap_or(f64::NAN)));
    }
    Ok(Value::String(s.to_string()))
}

// Find the byte index of the colon that separates a mapping key from its value:
// the first `:` that is at end-of-content or followed by whitespace, ignoring
// colons inside quotes. Returns None when the line is not a mapping entry.
fn find_key_colon(content: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut iter = content.char_indices().peekable();
    while let Some((i, ch)) = iter.next() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                } else if ch == ':'
                    && iter.peek().is_none_or(|&(_, next)| next.is_whitespace())
                {
                    return Some(i);
                }
            }
        }
    }
    None
}

fn parse_key(text: &str, line: usize) -> Result<String, String> {
    let s = text.trim();
    if s.is_empty() {
        return Err(yaml_error("empty mapping key", Some(line)));
    }
    if s.starts_with('"') || s.starts_with('\'') {
        return match parse_scalar(s, line)? {
            Value::String(k) => Ok(k),
            other => Ok(js_display(&other)),
        };
    }
    Ok(s.to_string())
}

// Stringify a value the way JS template interpolation would (only reached for
// unusual quoted keys; mappings key on strings).
fn js_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn is_seq_item(content: &str) -> bool {
    content == "-" || content.starts_with("- ")
}

// Recursive descent over the token list. Each parse_* returns (value, next index).
fn parse_block(nodes: &[Node], i: usize, indent: usize) -> Result<(Value, usize), String> {
    if i >= nodes.len() || nodes[i].indent < indent {
        return Ok((Value::Null, i));
    }
    if is_seq_item(&nodes[i].content) {
        parse_seq(nodes, i, nodes[i].indent)
    } else {
        parse_map(nodes, i, nodes[i].indent)
    }
}

fn parse_map(nodes: &[Node], mut i: usize, indent: usize) -> Result<(Value, usize), String> {
    let mut obj = Map::new();
    while i < nodes.len() && nodes[i].indent == indent && !is_seq_item(&nodes[i].content) {
        let Node { content, line, .. } = &nodes[i];
        let colon = find_key_colon(content).ok_or_else(|| {
            yaml_error(&format!("expected \"key: value\", got \"{}\"", content), Some(*line))
        })?;
        let key = parse_key(&content[..colon], *line)?;
        let after = content[colon + 1..].trim();
        if !after.is_empty() {
            obj.insert(key, parse_scalar(after, *line)?);
            i += 1;
        } else {
            // Value is a nested block on following lines. It may be indented deeper
            // (a mapping) or a sequence at the same indent as the key.
            i += 1;
            match nodes.get(i) {
                Some(next)
                    if next.indent > indent
                        || (next.indent == indent && is_seq_item(&next.content)) =>
                {
                    let (child, ni) = parse_block(nodes, i, next.indent)?;
                    obj.insert(key, child);
                    i = ni;
                }
                _ => {
                    obj.insert(key, Value::Null);
                }
            }
        }
    }
    Ok((Value::Object(obj), i))
}

fn parse_seq(nodes: &[Node], mut i: usize, indent: usize) -> Result<(Value, usize), String> {
    let mut arr = Vec::new();
    while i < nodes.len() && nodes[i].indent == indent && is_seq_item(&nodes[i].content) {
        let Node { content, line, .. } = &nodes[i];
        if content == "-" {
            // Nested block begins on the following deeper lines.
            i += 1;
            match nodes.get(i) {
                Some(next) if next.indent > indent => {
                    let (child, ni) = parse_block(nodes, i, next.indent)?;
                    arr.push(child);
                    i = ni;
                }
                _ => arr.push(Value::Null),
            }
            continue;
        }
        let rest = &content[2..]; // after "- "
        let offset = content.len() - rest.len(); // column of `rest` within content
        if find_key_colon(rest).is_some() {
            // Inline mapping item: rewrite the dash line as a normal mapping line at
            // the deeper indent, then let parse_map consume it plus its continuation
            // lines. The next dash sits at `indent` (< map_indent) so parse_map stops.
            let map_indent = indent + offset;
            let mut view = nodes.to_vec();
            view[i] = Node {
                indent: map_indent,
                content: rest.to_string(),
                line: *line,
            };
            let (child, ni) = parse_map(&view, i, map_indent)?;
            arr.push(child);
            i = ni;
        } else {
            arr.push(parse_scalar(rest, *line)?);
            i += 1;
        }
    }
    Ok((Value::Array(arr), i))
}

pub fn parse_yaml(text: &str) -> Result<Value, String> {
    let nodes = tokenize(text)?;
    if nodes.is_empty() {
        return Ok(Value::Null);
    }
    let (value, end) = parse_block(&nodes, 0, nodes[0].indent)?;
    if end != nodes.len() {
        return Err(yaml_error(
            "unexpected indentation",
            nodes.get(end).map(|n| n.line),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_scalars_strings_bools_null_numbers() {
        let doc = parse_yaml(
            &["a: hello", "b: true", "c: false", "d: null", "e: ~", "f: 42", "g: 0.5", "h: -3"]
                .join("\n"),
        )
        .unwrap();
        assert_eq!(
            doc,
            json!({
                "a": "hello",
                "b": true,
                "c": false,
                "d": null,
                "e": null,
                "f": 42,
                "g": 0.5,
                "h": -3,
            })
        );
    }

    #[test]
    fn quoted_strings_keep_their_literal_value() {
        let doc = parse_yaml(
            &["s: \"true\"", "t: 'mise run setup'", "u: \"a: b # c\"", "v: 'it''s ok'"].join("\n"),
        )
        .unwrap();
        assert_eq!(doc["s"], "true"); // quoted -> string, not boolean
        assert_eq!(doc["t"], "mise run setup");
        assert_eq!(doc["u"], "a: b # c"); // colon and # inside quotes are literal
        assert_eq!(doc["v"], "it's ok"); // '' -> '
    }

    #[test]
    fn strips_comments_but_not_inside_quotes() {
        let doc = parse_yaml(
            &["# full line comment", "a: 1 # trailing", "b: \"x # y\" # real comment"].join("\n"),
        )
        .unwrap();
        assert_eq!(doc, json!({ "a": 1, "b": "x # y" }));
    }

    #[test]
    fn nested_mappings_via_indentation() {
        let doc =
            parse_yaml(&["setup:", "  command: mise run setup", "  blocking: true"].join("\n"))
                .unwrap();
        assert_eq!(doc, json!({ "setup": { "command": "mise run setup", "blocking": true } }));
    }

    #[test]
    fn sequence_of_scalars() {
        let doc = parse_yaml(&["items:", "  - one", "  - two", "  - 3"].join("\n")).unwrap();
        assert_eq!(doc, json!({ "items": ["one", "two", 3] }));
    }

    #[test]
    fn sequence_of_mappings_with_continuation_lines() {
        let doc = parse_yaml(
            &[
                "tabs:",
                "  - title: main",
                "    panes:",
                "      - title: agent",
                "        command: opencode",
                "        setup: true",
                "      - title: editor",
                "        split: vertical",
                "  - title: dev",
                "    panes:",
                "      - title: server",
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(
            doc,
            json!({
                "tabs": [
                    {
                        "title": "main",
                        "panes": [
                            { "title": "agent", "command": "opencode", "setup": true },
                            { "title": "editor", "split": "vertical" },
                        ],
                    },
                    { "title": "dev", "panes": [{ "title": "server" }] },
                ],
            })
        );
    }

    #[test]
    fn parses_the_full_example_shape() {
        let doc = parse_yaml(
            &[
                "layouts:",
                "  - id: web-app",
                "    setup:",
                "      command: mise run setup",
                "      blocking: true",
                "    tabs:",
                "      - title: main",
                "        panes:",
                "          - title: agent",
                "            command: opencode",
                "            setup: true",
                "          - title: editor",
                "            command: nvim",
                "            split: vertical",
                "workspaces:",
                "  - path: ~/.herdr/worktrees/web-app",
                "    defaultLayout: web-app",
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(doc["layouts"].as_array().unwrap().len(), 1);
        assert_eq!(doc["layouts"][0]["id"], "web-app");
        assert_eq!(doc["layouts"][0]["setup"]["blocking"], true);
        assert_eq!(doc["layouts"][0]["tabs"][0]["panes"][0]["setup"], true);
        assert_eq!(doc["layouts"][0]["tabs"][0]["panes"][1]["split"], "vertical");
        assert_eq!(
            doc["workspaces"],
            json!([{ "path": "~/.herdr/worktrees/web-app", "defaultLayout": "web-app" }])
        );
    }

    #[test]
    fn empty_or_comment_only_document_is_null() {
        assert_eq!(parse_yaml("").unwrap(), Value::Null);
        assert_eq!(parse_yaml("# just a comment\n\n").unwrap(), Value::Null);
    }

    #[test]
    fn rejects_tab_indentation() {
        assert!(parse_yaml("a:\n\t- x").is_err());
    }
}
