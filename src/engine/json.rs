//! A minimal JSON reader, for the benchmark tooling (reference dumps).

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> &[Value] {
        match self {
            Value::Arr(a) => a,
            _ => &[],
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Num(n) => *n as i64,
            _ => 0,
        }
    }
}

pub fn parse(src: &str) -> Option<Value> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let v = value(&chars, &mut i)?;
    Some(v)
}

fn ws(c: &[char], i: &mut usize) {
    while *i < c.len() && c[*i].is_whitespace() {
        *i += 1;
    }
}

fn value(c: &[char], i: &mut usize) -> Option<Value> {
    ws(c, i);
    match *c.get(*i)? {
        'n' => {
            *i += 4;
            Some(Value::Null)
        }
        't' => {
            *i += 4;
            Some(Value::Bool(true))
        }
        'f' => {
            *i += 5;
            Some(Value::Bool(false))
        }
        '"' => string(c, i).map(Value::Str),
        '[' => {
            *i += 1;
            let mut out = Vec::new();
            loop {
                ws(c, i);
                if c.get(*i) == Some(&']') {
                    *i += 1;
                    break;
                }
                out.push(value(c, i)?);
                ws(c, i);
                match c.get(*i)? {
                    ',' => *i += 1,
                    ']' => {
                        *i += 1;
                        break;
                    }
                    _ => return None,
                }
            }
            Some(Value::Arr(out))
        }
        '{' => {
            *i += 1;
            let mut out = Vec::new();
            loop {
                ws(c, i);
                if c.get(*i) == Some(&'}') {
                    *i += 1;
                    break;
                }
                let k = string(c, i)?;
                ws(c, i);
                if c.get(*i) != Some(&':') {
                    return None;
                }
                *i += 1;
                out.push((k, value(c, i)?));
                ws(c, i);
                match c.get(*i)? {
                    ',' => *i += 1,
                    '}' => {
                        *i += 1;
                        break;
                    }
                    _ => return None,
                }
            }
            Some(Value::Obj(out))
        }
        _ => {
            let start = *i;
            while *i < c.len()
                && (c[*i].is_ascii_digit() || matches!(c[*i], '-' | '+' | '.' | 'e' | 'E'))
            {
                *i += 1;
            }
            c[start..*i]
                .iter()
                .collect::<String>()
                .parse()
                .ok()
                .map(Value::Num)
        }
    }
}

fn string(c: &[char], i: &mut usize) -> Option<String> {
    if c.get(*i) != Some(&'"') {
        return None;
    }
    *i += 1;
    let mut out = String::new();
    let mut high: Option<u32> = None;
    loop {
        let ch = *c.get(*i)?;
        *i += 1;
        match ch {
            '"' => break,
            '\\' => {
                let e = *c.get(*i)?;
                *i += 1;
                match e {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let hex: String = c.get(*i..*i + 4)?.iter().collect();
                        *i += 4;
                        let code = u32::from_str_radix(&hex, 16).ok()?;
                        if (0xD800..0xDC00).contains(&code) {
                            high = Some(code);
                        } else if (0xDC00..0xE000).contains(&code) {
                            if let Some(h) = high.take() {
                                out.push(char::from_u32(
                                    0x10000 + ((h - 0xD800) << 10) + (code - 0xDC00),
                                )?);
                            }
                        } else {
                            out.push(char::from_u32(code)?);
                        }
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_values() {
        let v = parse(r#"[["a\"b", null, 3, [true]], {"k": "é😀"}]"#).unwrap();
        let a = v.as_arr();
        assert_eq!(a[0].as_arr()[0].as_str(), Some("a\"b"));
        assert_eq!(a[0].as_arr()[1], Value::Null);
        assert_eq!(a[0].as_arr()[2].as_i64(), 3);
        assert_eq!(
            a[1],
            Value::Obj(vec![("k".into(), Value::Str("é😀".into()))])
        );
    }
}
