//! Minimal JSON helpers: a comment-tolerant read (for deciding whether a client's
//! config already holds our entry) and a set-one-key edit that keeps key order and
//! the file's indentation.
use serde_json::Value;

/// Parse JSON that may carry `//` and `/* */` comments and trailing commas.
pub fn parse_lenient(text: &str) -> Option<Value> {
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&strip_jsonc(text)).ok()
}

fn strip_jsonc(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '"' => {
                out.push('"');
                i += 1;
                while i < c.len() {
                    out.push(c[i]);
                    if c[i] == '\\' && i + 1 < c.len() {
                        out.push(c[i + 1]);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if c[i - 1] == '"' {
                        break;
                    }
                }
            }
            '/' if c.get(i + 1) == Some(&'/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    drop_trailing_commas(&out)
}

fn drop_trailing_commas(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if in_str {
            out.push(ch);
            if ch == '\\' && i + 1 < c.len() {
                out.push(c[i + 1]);
                i += 1;
            } else if ch == '"' {
                in_str = false;
            }
        } else if ch == '"' {
            in_str = true;
            out.push(ch);
        } else if ch == ',' {
            let next = c[i + 1..].iter().find(|x| !x.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                out.push(ch);
            }
        } else {
            out.push(ch);
        }
        i += 1;
    }
    out
}

/// The indent string of the first indented line, if any.
fn detect_indent(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let ws: String = l.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        (!ws.is_empty() && l[ws.len()..].starts_with('"')).then_some(ws)
    })
}

/// Serialize `v` the way `original` looks: its indent (compact if it had none) and
/// its trailing newline.
pub fn render_like(original: &str, v: &Value) -> String {
    use serde::Serialize;
    let mut body = match detect_indent(original) {
        Some(ind) if original.contains('\n') => {
            let mut buf = Vec::new();
            let fmt = serde_json::ser::PrettyFormatter::with_indent(ind.as_bytes());
            let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
            v.serialize(&mut ser).expect("json serializes");
            String::from_utf8(buf).expect("json is utf-8")
        }
        _ if original.contains('\n') => serde_json::to_string_pretty(v).expect("json serializes"),
        _ if original.trim().is_empty() => {
            serde_json::to_string_pretty(v).expect("json serializes")
        }
        _ => serde_json::to_string(v).expect("json serializes"),
    };
    if original.is_empty() || original.ends_with('\n') {
        body.push('\n');
    }
    body
}

// ---- byte-preserving single-key edit -------------------------------------

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// End (exclusive) of the string starting at `i` (which is a quote).
fn string_end(b: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// End (exclusive) of the JSON value starting at `i`.
fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'"' => string_end(b, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_end(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace()
            {
                j += 1;
            }
            Some(j)
        }
    }
}

struct Member {
    key: String,
    val: (usize, usize),
}

/// Members of the object whose `{` is at `open`, plus the index of its `}`.
fn members(text: &str, open: usize) -> Option<(Vec<Member>, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = skip_ws(b, open + 1);
    loop {
        match *b.get(i)? {
            b'}' => return Some((out, i)),
            b'"' => {
                let ke = string_end(b, i)?;
                let key: String = serde_json::from_str(&text[i..ke]).ok()?;
                i = skip_ws(b, ke);
                if *b.get(i)? != b':' {
                    return None;
                }
                let vs = skip_ws(b, i + 1);
                let ve = value_end(b, vs)?;
                out.push(Member { key, val: (vs, ve) });
                i = skip_ws(b, ve);
                if *b.get(i)? == b',' {
                    i = skip_ws(b, i + 1);
                }
            }
            _ => return None,
        }
    }
}

/// Set `path[..-1]` objects (created if missing) and the final key to `value`
/// by splicing text, so every other byte of `text` stays. `None` if the text
/// is not a JSON object shaped as the path needs.
pub fn splice_set(text: &str, path: &[&str], value: &Value) -> Option<String> {
    let unit = detect_indent(text).unwrap_or_else(|| "  ".into());
    let multiline = text.contains('\n');
    let root = skip_ws(text.as_bytes(), 0);
    if text.as_bytes().get(root) != Some(&b'{') {
        return None;
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    splice_in(text, root, path, 0, value, &(unit, eol), multiline)
}

fn render_value(v: &Value, ue: &(String, &str), multiline: bool, depth: usize) -> String {
    use serde::Serialize;
    let (unit, eol) = (ue.0.as_str(), ue.1);
    if !multiline {
        return serde_json::to_string(v).expect("json serializes");
    }
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    v.serialize(&mut ser).expect("json serializes");
    String::from_utf8(buf)
        .expect("json is utf-8")
        .replace('\n', &format!("{eol}{}", unit.repeat(depth)))
}

fn splice_in(
    text: &str,
    open: usize,
    path: &[&str],
    depth: usize,
    value: &Value,
    ue: &(String, &str),
    multiline: bool,
) -> Option<String> {
    let (unit, eol) = (ue.0.as_str(), ue.1);
    let (ms, close) = members(text, open)?;
    let key = path[0];
    let leaf = path.len() == 1;
    let nested_value = |v: &Value| -> Value {
        path[1..]
            .iter()
            .rev()
            .fold(v.clone(), |acc, k| serde_json::json!({ *k: acc }))
    };
    if let Some(m) = ms.iter().find(|m| m.key == key) {
        if leaf {
            let new = render_value(value, ue, multiline, depth + 1);
            return Some(format!("{}{}{}", &text[..m.val.0], new, &text[m.val.1..]));
        }
        if text.as_bytes()[m.val.0] != b'{' {
            return None;
        }
        return splice_in(text, m.val.0, &path[1..], depth + 1, value, ue, multiline);
    }
    // Key absent: append a member (with any missing parents) to this object.
    let v = if leaf {
        value.clone()
    } else {
        nested_value(value)
    };
    let kj = serde_json::to_string(key).ok()?;
    let sep = if multiline { ": " } else { ":" };
    let member = format!("{kj}{sep}{}", render_value(&v, ue, multiline, depth + 1));
    let pad = if multiline {
        format!("{eol}{}", unit.repeat(depth + 1))
    } else {
        String::new()
    };
    match ms.last() {
        Some(last) => {
            let at = last.val.1;
            Some(format!("{},{pad}{member}{}", &text[..at], &text[at..]))
        }
        None => {
            let tail = if multiline {
                format!("{eol}{}", unit.repeat(depth))
            } else {
                String::new()
            };
            Some(format!(
                "{}{pad}{member}{tail}{}",
                &text[..=open],
                &text[close..]
            ))
        }
    }
}
