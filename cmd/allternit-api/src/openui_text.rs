//! Interactive answers (```openui fences) as plain text for places that can't draw them.
//!
//! An assistant reply in the Allternit app may carry a card written in OpenUI Lang inside a
//! fenced block tagged `openui` (the library is `allternit-ai` `src/lib/openui/answers-library.tsx`).
//! Slack, Telegram, Discord, WhatsApp, Teams, SMS, email and voice can't render it, so every
//! outbound channel path runs the reply through [`to_plain_text`] (or [`to_spoken_text`] /
//! [`SpokenFilter`] for speech). Raw OpenUI Lang never reaches a channel.
//!
//! The parser is deliberately tolerant: unknown or malformed lines are dropped, `$state`
//! defaults are used for expressions (`$total / $people`), and a card that yields nothing
//! readable becomes a one-line pointer to the app.

use std::collections::HashMap;

/// Shown when a card can't be turned into text.
pub const CARD_FALLBACK: &str = "(This reply has an interactive card. Open it in Allternit to use it.)";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    Text,
    Speech,
}

/// True when `text` holds an ```openui fence.
pub fn has_fence(text: &str) -> bool {
    text.lines().any(is_open_line)
}

/// The reply with every ```openui fence replaced by readable text. Text without a fence is returned unchanged.
pub fn to_plain_text(text: &str) -> String {
    convert(text, Style::Text)
}

/// Like [`to_plain_text`], worded for a voice: no table bars, options read as a sentence.
pub fn to_spoken_text(text: &str) -> String {
    convert(text, Style::Speech)
}

/// ```openui, or ```openui-lang (OpenUI's own prompt sections use that tag; the app accepts both).
fn is_open_line(line: &str) -> bool {
    matches!(line.trim(), "```openui" | "```openui-lang")
}

fn is_close_line(line: &str) -> bool {
    line.trim() == "```"
}

fn convert(text: &str, style: Style) -> String {
    if !has_fence(text) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<String> = None;
    for line in text.split_inclusive('\n') {
        match fence.as_mut() {
            None if is_open_line(line) => fence = Some(String::new()),
            None => out.push_str(line),
            Some(_) if is_close_line(line) => {
                let code = fence.take().unwrap_or_default();
                push_block(&mut out, &card_text(&code, style));
            }
            Some(code) => code.push_str(line),
        }
    }
    // An unclosed fence (a cut-off reply) is still a card, not text to show.
    if let Some(code) = fence {
        push_block(&mut out, &card_text(&code, style));
    }
    tidy(&out)
}

fn push_block(out: &mut String, block: &str) {
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
    }
    out.push_str(block.trim_end());
    out.push_str("\n\n");
}

/// Collapse runs of blank lines and trim the ends.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.trim().lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// One card's OpenUI Lang as text; [`CARD_FALLBACK`] when nothing readable comes out.
pub fn card_text(code: &str, style: Style) -> String {
    let program = Program::parse(code);
    let rendered = program.render(style);
    if rendered.trim().is_empty() {
        CARD_FALLBACK.to_string()
    } else {
        rendered
    }
}

// ---------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Str(String),
    Num(f64),
    Ident(String),
    Var(String),
    Builtin(String),
    Punct(&'static str),
}

const PUNCTS: [&str; 24] = [
    "==", "!=", ">=", "<=", "&&", "||", "(", ")", "[", "]", "{", "}", ",", ":", "+", "-", "*", "/", "%", ">", "<", "!", "?", ".",
];

fn lex(src: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            let mut s = String::new();
            i += 1;
            loop {
                let ch = *chars.get(i)?;
                i += 1;
                if ch == quote {
                    break;
                }
                if ch == '\\' {
                    let esc = *chars.get(i)?;
                    i += 1;
                    s.push(match esc {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    s.push(ch);
                }
            }
            out.push(Tok::Str(s));
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit())) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_') {
                i += 1;
            }
            let raw: String = chars[start..i].iter().filter(|c| **c != '_').collect();
            out.push(Tok::Num(raw.parse().ok()?));
            continue;
        }
        if c == '$' || c == '@' || c.is_alphabetic() || c == '_' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            out.push(match c {
                '$' => Tok::Var(word),
                '@' => Tok::Builtin(word[1..].to_string()),
                _ => Tok::Ident(word),
            });
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
        let p = PUNCTS.iter().find(|p| rest.starts_with(**p))?;
        out.push(Tok::Punct(p));
        i += p.chars().count();
    }
    Some(out)
}

// ---------------------------------------------------------------- parser

#[derive(Clone, Debug)]
enum Node {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
    Arr(Vec<Node>),
    Obj(Vec<(String, Node)>),
    Call(String, Vec<Node>),
    Builtin(String, Vec<Node>),
    Ref(String),
    Var(String),
    Unary(&'static str, Box<Node>),
    Binary(&'static str, Box<Node>, Box<Node>),
    Ternary(Box<Node>, Box<Node>, Box<Node>),
    Member(Box<Node>, String),
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, p: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(q)) if *q == p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str) -> Option<()> {
        self.eat(p).then_some(())
    }

    fn expr(&mut self) -> Option<Node> {
        let cond = self.binary(0)?;
        if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            return Some(Node::Ternary(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Some(cond)
    }

    fn binary(&mut self, min: u8) -> Option<Node> {
        let mut left = self.unary()?;
        loop {
            let (op, prec) = match self.peek() {
                Some(Tok::Punct(p)) => match *p {
                    "||" => ("||", 1),
                    "&&" => ("&&", 2),
                    "==" | "!=" => (*p, 3),
                    ">" | "<" | ">=" | "<=" => (*p, 4),
                    "+" | "-" => (*p, 5),
                    "*" | "/" | "%" => (*p, 6),
                    _ => break,
                },
                _ => break,
            };
            if prec < min {
                break;
            }
            self.pos += 1;
            let right = self.binary(prec + 1)?;
            left = Node::Binary(op, Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn unary(&mut self) -> Option<Node> {
        if self.eat("!") {
            return Some(Node::Unary("!", Box::new(self.unary()?)));
        }
        if self.eat("-") {
            return Some(Node::Unary("-", Box::new(self.unary()?)));
        }
        let mut node = self.primary()?;
        while self.eat(".") {
            match self.peek().cloned() {
                Some(Tok::Ident(f)) => {
                    self.pos += 1;
                    node = Node::Member(Box::new(node), f);
                }
                _ => return None,
            }
        }
        Some(node)
    }

    fn list(&mut self, close: &str) -> Option<Vec<Node>> {
        let mut items = Vec::new();
        if self.eat(close) {
            return Some(items);
        }
        loop {
            items.push(self.expr()?);
            if self.eat(close) {
                return Some(items);
            }
            self.expect(",")?;
            // Trailing comma.
            if self.eat(close) {
                return Some(items);
            }
        }
    }

    fn primary(&mut self) -> Option<Node> {
        let tok = self.peek()?.clone();
        self.pos += 1;
        Some(match tok {
            Tok::Str(s) => Node::Str(s),
            Tok::Num(n) => Node::Num(n),
            Tok::Var(v) => Node::Var(v),
            Tok::Builtin(name) => {
                self.expect("(")?;
                Node::Builtin(name, self.list(")")?)
            }
            Tok::Ident(id) => match id.as_str() {
                "true" => Node::Bool(true),
                "false" => Node::Bool(false),
                "null" | "undefined" => Node::Null,
                _ if self.eat("(") => Node::Call(id, self.list(")")?),
                _ => Node::Ref(id),
            },
            Tok::Punct("(") => {
                let e = self.expr()?;
                self.expect(")")?;
                e
            }
            Tok::Punct("[") => Node::Arr(self.list("]")?),
            Tok::Punct("{") => {
                let mut fields = Vec::new();
                if !self.eat("}") {
                    loop {
                        let key = match self.peek()?.clone() {
                            Tok::Str(s) | Tok::Ident(s) => s,
                            _ => return None,
                        };
                        self.pos += 1;
                        self.expect(":")?;
                        fields.push((key, self.expr()?));
                        if self.eat("}") {
                            break;
                        }
                        self.expect(",")?;
                        if self.eat("}") {
                            break;
                        }
                    }
                }
                Node::Obj(fields)
            }
            _ => return None,
        })
    }
}

fn parse_expr(src: &str) -> Option<Node> {
    let mut p = Parser { toks: lex(src)?, pos: 0 };
    let node = p.expr()?;
    (p.pos == p.toks.len()).then_some(node)
}

/// Split the code into `name = expr` statements. A statement may continue onto the next
/// line while brackets are open or a string is unterminated.
fn statements(code: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth: i32 = 0;
    for line in code.lines() {
        let trimmed = line.trim();
        if cur.is_empty() && (trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#')) {
            continue;
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
        depth += bracket_delta(line);
        if depth <= 0 {
            if let Some((name, expr)) = split_statement(&cur) {
                out.push((name, expr));
            }
            cur.clear();
            depth = 0;
        }
    }
    if !cur.is_empty() {
        if let Some((name, expr)) = split_statement(&cur) {
            out.push((name, expr));
        }
    }
    out
}

fn bracket_delta(line: &str) -> i32 {
    let mut d = 0;
    let mut quote: Option<char> = None;
    let mut esc = false;
    for c in line.chars() {
        if let Some(q) = quote {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' | '{' => d += 1,
            ')' | ']' | '}' => d -= 1,
            _ => {}
        }
    }
    d
}

fn split_statement(s: &str) -> Option<(String, String)> {
    let eq = s.find('=')?;
    let name = s[..eq].trim();
    // `==` would be an expression, not an assignment.
    if s[eq + 1..].starts_with('=') {
        return None;
    }
    let valid = name.chars().enumerate().all(|(i, c)| c.is_alphanumeric() || c == '_' || (i == 0 && c == '$'));
    if name.is_empty() || !valid {
        return None;
    }
    Some((name.to_string(), s[eq + 1..].trim().to_string()))
}

// ---------------------------------------------------------------- values

#[derive(Clone, Debug)]
enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Val>),
    Obj(Vec<(String, Val)>),
    Comp(String, Vec<Val>),
}

impl Val {
    fn num(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            Val::Str(s) => s.trim().parse().ok(),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Val::Null => false,
            Val::Bool(b) => *b,
            Val::Num(n) => *n != 0.0,
            Val::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    fn text(&self) -> String {
        match self {
            Val::Null => String::new(),
            Val::Bool(b) => if *b { "Yes" } else { "No" }.into(),
            Val::Num(n) => fmt_number(*n, None),
            Val::Str(s) => s.clone(),
            Val::Arr(items) => items.iter().map(Val::text).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
            Val::Obj(fields) => fields.iter().map(|(k, v)| format!("{k}: {}", v.text())).collect::<Vec<_>>().join(", "),
            Val::Comp(..) => String::new(),
        }
    }

    fn items(&self) -> Vec<Val> {
        match self {
            Val::Arr(v) => v.clone(),
            Val::Null => Vec::new(),
            other => vec![other.clone()],
        }
    }

    fn field(&self, key: &str) -> Val {
        match self {
            Val::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or(Val::Null),
            // Column pluck: `rows.title`.
            Val::Arr(items) => Val::Arr(items.iter().map(|i| i.field(key)).collect()),
            _ => Val::Null,
        }
    }
}

struct Program {
    stmts: HashMap<String, Node>,
    order: Vec<String>,
}

impl Program {
    fn parse(code: &str) -> Self {
        let mut stmts = HashMap::new();
        let mut order = Vec::new();
        for (name, src) in statements(code) {
            if let Some(node) = parse_expr(&src) {
                if !stmts.contains_key(&name) {
                    order.push(name.clone());
                }
                stmts.insert(name, node);
            }
        }
        Program { stmts, order }
    }

    fn eval(&self, node: &Node, depth: u32) -> Val {
        if depth > 64 {
            return Val::Null;
        }
        let ev = |n: &Node| self.eval(n, depth + 1);
        match node {
            Node::Str(s) => Val::Str(s.clone()),
            Node::Num(n) => Val::Num(*n),
            Node::Bool(b) => Val::Bool(*b),
            Node::Null => Val::Null,
            Node::Arr(items) => Val::Arr(items.iter().map(ev).collect()),
            Node::Obj(fields) => Val::Obj(fields.iter().map(|(k, v)| (k.clone(), ev(v))).collect()),
            Node::Call(name, args) => {
                if name.chars().next().is_some_and(char::is_uppercase) {
                    Val::Comp(name.clone(), args.iter().map(ev).collect())
                } else {
                    // Query()/Mutation() and other lowercase calls: no data here.
                    Val::Null
                }
            }
            Node::Builtin(name, args) => builtin(name, &args.iter().map(ev).collect::<Vec<_>>()),
            Node::Ref(name) | Node::Var(name) => self.stmts.get(name).map(|n| self.eval(n, depth + 1)).unwrap_or(Val::Null),
            Node::Unary("!", a) => Val::Bool(!ev(a).truthy()),
            Node::Unary(_, a) => ev(a).num().map(|n| Val::Num(-n)).unwrap_or(Val::Null),
            Node::Binary(op, a, b) => binary(op, ev(a), ev(b)),
            Node::Ternary(c, a, b) => {
                if ev(c).truthy() {
                    ev(a)
                } else {
                    ev(b)
                }
            }
            Node::Member(obj, field) => ev(obj).field(field),
        }
    }

    fn root(&self) -> Option<Val> {
        if let Some(n) = self.stmts.get("root") {
            return Some(self.eval(n, 0));
        }
        // No `root`: the last component statement nobody else references.
        self.order
            .iter()
            .rev()
            .filter(|name| !name.starts_with('$'))
            .map(|name| self.eval(&self.stmts[name], 0))
            .find(|v| matches!(v, Val::Comp(..)))
    }

    fn render(&self, style: Style) -> String {
        let mut r = Renderer { style, blocks: Vec::new(), options: Vec::new() };
        if let Some(root) = self.root() {
            r.node(&root);
        }
        r.finish()
    }
}

fn binary(op: &str, a: Val, b: Val) -> Val {
    match op {
        "&&" => if a.truthy() { b } else { a },
        "||" => if a.truthy() { a } else { b },
        "==" => Val::Bool(eq(&a, &b)),
        "!=" => Val::Bool(!eq(&a, &b)),
        // JavaScript-like: a string on either side concatenates.
        "+" if matches!(a, Val::Str(_)) || matches!(b, Val::Str(_)) => Val::Str(format!("{}{}", a.text(), b.text())),
        "+" => match (a.num(), b.num()) {
            (Some(x), Some(y)) => Val::Num(x + y),
            _ => Val::Null,
        },
        _ => match (a.num(), b.num()) {
            (Some(x), Some(y)) => match op {
                "-" => Val::Num(x - y),
                "*" => Val::Num(x * y),
                "/" if y != 0.0 => Val::Num(x / y),
                "%" if y != 0.0 => Val::Num(x % y),
                ">" => Val::Bool(x > y),
                "<" => Val::Bool(x < y),
                ">=" => Val::Bool(x >= y),
                "<=" => Val::Bool(x <= y),
                _ => Val::Null,
            },
            _ => Val::Null,
        },
    }
}

fn eq(a: &Val, b: &Val) -> bool {
    match (a.num(), b.num(), a, b) {
        (_, _, Val::Str(x), Val::Str(y)) => x == y,
        (Some(x), Some(y), _, _) => x == y,
        (_, _, Val::Null, Val::Null) => true,
        _ => a.text() == b.text(),
    }
}

fn builtin(name: &str, args: &[Val]) -> Val {
    let first = args.first().cloned().unwrap_or(Val::Null);
    let nums = || first.items().iter().filter_map(Val::num).collect::<Vec<f64>>();
    let one = |f: fn(f64) -> f64| first.num().map(|n| Val::Num(f(n))).unwrap_or(Val::Null);
    match name {
        "Count" => Val::Num(first.items().len() as f64),
        "Sum" => Val::Num(nums().iter().sum()),
        "Avg" => {
            let v = nums();
            if v.is_empty() {
                Val::Null
            } else {
                Val::Num(v.iter().sum::<f64>() / v.len() as f64)
            }
        }
        "Min" => nums().into_iter().reduce(f64::min).map(Val::Num).unwrap_or(Val::Null),
        "Max" => nums().into_iter().reduce(f64::max).map(Val::Num).unwrap_or(Val::Null),
        "First" => first.items().first().cloned().unwrap_or(Val::Null),
        "Last" => first.items().last().cloned().unwrap_or(Val::Null),
        "Abs" => one(f64::abs),
        "Floor" => one(f64::floor),
        "Ceil" => one(f64::ceil),
        "Round" => {
            let digits = args.get(1).and_then(Val::num).unwrap_or(0.0).clamp(0.0, 10.0) as i32;
            let m = 10f64.powi(digits);
            first.num().map(|n| Val::Num((n * m).round() / m)).unwrap_or(Val::Null)
        }
        _ => Val::Null,
    }
}

// ---------------------------------------------------------------- formatting

fn group_thousands(int: &str) -> String {
    let (sign, digits) = int.strip_prefix('-').map(|d| ("-", d)).unwrap_or(("", int));
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

fn fmt_number(n: f64, decimals: Option<usize>) -> String {
    if !n.is_finite() {
        return String::new();
    }
    let digits = decimals.unwrap_or(if n.fract() == 0.0 { 0 } else { 2 });
    let s = format!("{n:.digits$}");
    let (int, frac) = s.split_once('.').map(|(i, f)| (i.to_string(), Some(f.to_string()))).unwrap_or((s.clone(), None));
    let grouped = group_thousands(&int);
    match frac {
        Some(f) if decimals.is_none() => {
            let f = f.trim_end_matches('0');
            if f.is_empty() {
                grouped
            } else {
                format!("{grouped}.{f}")
            }
        }
        Some(f) => format!("{grouped}.{f}"),
        None => grouped,
    }
}

/// A value the way the card shows it: currency, percent, number or as-is.
fn fmt_value(v: &Val, format: Option<&str>, unit: Option<&str>) -> String {
    let n = match v {
        Val::Num(n) => Some(*n),
        Val::Str(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    let Some(n) = n else {
        let t = v.text();
        return if t.is_empty() { "—".into() } else { t };
    };
    match format {
        Some("currency") => {
            let code = unit.filter(|u| u.len() == 3 && u.chars().all(|c| c.is_ascii_uppercase())).unwrap_or("USD");
            let amount = fmt_number(n.abs(), Some(2));
            let sign = if n < 0.0 { "-" } else { "" };
            match code {
                "USD" => format!("{sign}${amount}"),
                "EUR" => format!("{sign}€{amount}"),
                "GBP" => format!("{sign}£{amount}"),
                other => format!("{sign}{amount} {other}"),
            }
        }
        Some("percent") => format!("{}%", fmt_number(n, None)),
        _ => match unit.filter(|u| !u.is_empty()) {
            Some(u) => format!("{} {u}", fmt_number(n, None)),
            None => fmt_number(n, None),
        },
    }
}

// ---------------------------------------------------------------- renderer

struct Renderer {
    style: Style,
    blocks: Vec<String>,
    /// Button labels and Choice options, numbered together at the end.
    options: Vec<String>,
}

fn arg(args: &[Val], i: usize) -> Val {
    args.get(i).cloned().unwrap_or(Val::Null)
}

fn opt_str(v: &Val) -> Option<String> {
    match v {
        Val::Str(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

impl Renderer {
    fn node(&mut self, v: &Val) {
        match v {
            Val::Comp(name, args) => self.component(name, args),
            Val::Arr(items) => items.iter().for_each(|i| self.node(i)),
            Val::Str(s) if !s.trim().is_empty() => self.blocks.push(s.trim().to_string()),
            _ => {}
        }
    }

    fn children_of(&mut self, args: &[Val]) {
        for a in args {
            if matches!(a, Val::Arr(_) | Val::Comp(..)) {
                self.node(a);
            }
        }
    }

    fn component(&mut self, name: &str, args: &[Val]) {
        let s = |i: usize| opt_str(&arg(args, i));
        match name {
            // Answer(title?, children): tolerate the title being left out.
            "Answer" | "Root" | "Card" | "Stack" => {
                if let Some(title) = args.iter().find_map(opt_str) {
                    self.blocks.push(title);
                }
                self.children_of(args);
            }
            "Row" => self.children_of(args),
            "Text" => {
                if let Some(t) = s(0) {
                    self.blocks.push(t);
                }
            }
            "Metric" => {
                let label = s(0).unwrap_or_default();
                let value = fmt_value(&arg(args, 1), s(2).as_deref(), s(3).as_deref());
                let line = match (label.is_empty(), s(4)) {
                    (true, _) => value,
                    (false, Some(note)) => format!("{label}: {value} ({note})"),
                    (false, None) => format!("{label}: {value}"),
                };
                self.blocks.push(line);
            }
            "Table" => self.table(&arg(args, 0), &arg(args, 1)),
            "BarChart" => {
                let (labels, values) = (arg(args, 0).items(), arg(args, 1).items());
                let (format, unit) = (s(2), s(3));
                let lines: Vec<String> = labels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{}: {}", l.text(), fmt_value(values.get(i).unwrap_or(&Val::Null), format.as_deref(), unit.as_deref())))
                    .collect();
                if !lines.is_empty() {
                    self.blocks.push(lines.join("\n"));
                }
            }
            "Steps" => {
                let lines: Vec<String> = arg(args, 0).items().iter().enumerate().map(|(i, v)| format!("{}. {}", i + 1, v.text())).collect();
                if !lines.is_empty() {
                    self.blocks.push(lines.join("\n"));
                }
            }
            "Compare" => {
                let lines: Vec<String> = arg(args, 0)
                    .items()
                    .iter()
                    .map(|col| {
                        let points = col.field("points").items().iter().map(Val::text).collect::<Vec<_>>().join("; ");
                        format!("{}: {points}", col.field("title").text())
                    })
                    .collect();
                if !lines.is_empty() {
                    self.blocks.push(lines.join("\n"));
                }
            }
            "Callout" => {
                if let Some(t) = s(0) {
                    let lead = match s(1).as_deref() {
                        Some("warning") | Some("danger") => "Warning",
                        _ => "Note",
                    };
                    self.blocks.push(format!("{lead}: {t}"));
                }
            }
            "Slider" => {
                let label = s(0).unwrap_or_default();
                let value = fmt_value(&arg(args, 1), s(5).as_deref(), s(6).as_deref());
                let range = match (arg(args, 2).num(), arg(args, 3).num()) {
                    (Some(min), Some(max)) => format!(" (from {} to {})", fmt_number(min, None), fmt_number(max, None)),
                    _ => String::new(),
                };
                self.blocks.push(format!("{label}: {value}{range}"));
            }
            "NumberField" | "Select" | "Toggle" => {
                let label = s(0).unwrap_or_default();
                let value = arg(args, 1);
                let shown = if let Val::Bool(b) = value { if b { "On" } else { "Off" }.to_string() } else { fmt_value(&value, None, None) };
                self.blocks.push(format!("{label}: {shown}"));
            }
            "Button" => {
                if let Some(label) = s(0) {
                    self.options.push(label);
                }
            }
            "Choice" => {
                if let Some(q) = s(0) {
                    self.blocks.push(q);
                }
                self.options.extend(arg(args, 1).items().iter().map(Val::text).filter(|t| !t.is_empty()));
            }
            // Unknown component: its strings and children still carry the meaning.
            _ => {
                for a in args {
                    match a {
                        Val::Str(t) if !t.trim().is_empty() => self.blocks.push(t.trim().to_string()),
                        Val::Arr(_) | Val::Comp(..) => self.node(a),
                        _ => {}
                    }
                }
            }
        }
    }

    fn table(&mut self, headers: &Val, rows: &Val) {
        let headers: Vec<String> = headers.items().iter().map(Val::text).collect();
        let rows: Vec<Vec<String>> = rows.items().iter().map(|r| r.items().iter().map(|c| fmt_value(c, None, None)).collect()).collect();
        if headers.is_empty() && rows.is_empty() {
            return;
        }
        let lines: Vec<String> = match self.style {
            Style::Speech => rows
                .iter()
                .map(|r| {
                    r.iter()
                        .enumerate()
                        .map(|(i, c)| match headers.get(i).filter(|h| !h.is_empty()) {
                            Some(h) if i > 0 => format!("{h} {c}"),
                            _ => c.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect(),
            Style::Text => std::iter::once(headers.join(" | ")).filter(|h| !h.trim().is_empty()).chain(rows.iter().map(|r| r.join(" | "))).collect(),
        };
        self.blocks.push(lines.join("\n"));
    }

    fn finish(mut self) -> String {
        if !self.options.is_empty() {
            match self.style {
                Style::Text => {
                    let list: Vec<String> = self.options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
                    self.blocks.push(format!("{}\nReply with a number or the option.", list.join("\n")));
                }
                Style::Speech => {
                    let mut list = self.options.clone();
                    let last = list.pop().unwrap_or_default();
                    let joined = if list.is_empty() { last } else { format!("{}, or {last}", list.join(", ")) };
                    self.blocks.push(format!("You can say {joined}."));
                }
            }
        }
        self.blocks.join("\n\n")
    }
}

// ---------------------------------------------------------------- streaming (voice)

/// Streaming version for speech: text outside a fence passes through as it arrives; a fence
/// is held until it closes and then spoken as [`to_spoken_text`] would.
#[derive(Default)]
pub struct SpokenFilter {
    /// Start of a line that might still become a fence marker.
    pending: String,
    fence: Option<String>,
    /// Part of the current line was already passed on, so it can't open a fence.
    mid_line: bool,
}

impl SpokenFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; returns what may be spoken now.
    pub fn push(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        for c in chunk.chars() {
            self.pending.push(c);
            if c == '\n' {
                let line = std::mem::take(&mut self.pending);
                if self.mid_line {
                    out.push_str(&line);
                } else {
                    self.line(&line, &mut out);
                }
                self.mid_line = false;
            } else if self.fence.is_none() && (self.mid_line || !could_be_fence_start(&self.pending)) {
                out.push_str(&std::mem::take(&mut self.pending));
                self.mid_line = true;
            }
        }
        out
    }

    fn line(&mut self, line: &str, out: &mut String) {
        match self.fence.as_mut() {
            None if is_open_line(line) => self.fence = Some(String::new()),
            None => out.push_str(line),
            Some(_) if is_close_line(line) => {
                let code = self.fence.take().unwrap_or_default();
                out.push_str(&card_text(&code, Style::Speech));
                out.push('\n');
            }
            Some(code) => code.push_str(line),
        }
    }

    /// The end of the reply: whatever is still held.
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        let rest = std::mem::take(&mut self.pending);
        let mid_line = std::mem::replace(&mut self.mid_line, false);
        match self.fence.take() {
            Some(mut code) => {
                if !is_close_line(&rest) {
                    code.push_str(&rest);
                }
                out.push_str(&card_text(&code, Style::Speech));
            }
            None if !mid_line && is_open_line(&rest) => out.push_str(CARD_FALLBACK),
            None => out.push_str(&rest),
        }
        out
    }
}

/// True while `partial` (the start of a line) could still turn into "```openui".
fn could_be_fence_start(partial: &str) -> bool {
    let t = partial.trim_start();
    t.is_empty() || "```openui".starts_with(t) || t.starts_with("```openui")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALC: &str = "Here's a quick calculator.\n\n```openui\n$people = 4\n$total = 120\nroot = Answer(\"Split the bill\", [inputs, per])\ninputs = Row([Slider(\"People\", $people, 1, 12), NumberField(\"Total\", $total)])\nper = Metric(\"Each person pays\", $total / $people, \"currency\", \"USD\", \"from your input\")\n```\n\nChange the numbers in the app to recalculate.";

    #[test]
    fn calculator_uses_state_defaults() {
        let t = to_plain_text(CALC);
        assert!(!t.contains("```"), "{t}");
        assert!(!t.contains("$people"), "{t}");
        assert!(t.starts_with("Here's a quick calculator."), "{t}");
        assert!(t.contains("Split the bill"), "{t}");
        assert!(t.contains("People: 4 (from 1 to 12)"), "{t}");
        assert!(t.contains("Total: 120"), "{t}");
        assert!(t.contains("Each person pays: $30.00 (from your input)"), "{t}");
        assert!(t.ends_with("Change the numbers in the app to recalculate."), "{t}");
    }

    #[test]
    fn choice_becomes_numbered_options() {
        let src = "```openui\nroot = Answer([Choice(\"Which plan?\", [\"Plus\", \"Super\", \"Ultra\"]), Button(\"Compare all\")])\n```";
        let t = to_plain_text(src);
        assert_eq!(t, "Which plan?\n\n1. Plus\n2. Super\n3. Ultra\n4. Compare all\nReply with a number or the option.");
        let spoken = to_spoken_text(src);
        assert_eq!(spoken, "Which plan?\n\nYou can say Plus, Super, Ultra, or Compare all.");
    }

    #[test]
    fn table_steps_compare_callout() {
        let src = "```openui\nroot = Answer(\"Options\", [t, s, c, n])\nt = Table([\"Plan\", \"Price\"], [[\"Plus\", 20], [\"Super\", 1250.5]])\ns = Steps([\"Sign in\", \"Pick a plan\"])\nc = Compare([{\"title\": \"A\", \"points\": [\"fast\", \"cheap\"]}, {title: \"B\", points: [\"slow\"]}])\nn = Callout(\"Prices are monthly.\", \"warning\")\n```";
        let t = to_plain_text(src);
        assert!(t.contains("Plan | Price\nPlus | 20\nSuper | 1,250.5"), "{t}");
        assert!(t.contains("1. Sign in\n2. Pick a plan"), "{t}");
        assert!(t.contains("A: fast; cheap\nB: slow"), "{t}");
        assert!(t.contains("Warning: Prices are monthly."), "{t}");
        let spoken = to_spoken_text(src);
        assert!(spoken.contains("Plus, Price 20"), "{spoken}");
        assert!(!spoken.contains('|'), "{spoken}");
    }

    #[test]
    fn multi_line_statements_and_bar_chart() {
        let src = "```openui\nroot = Answer(\"Spend\", [\n  BarChart([\"Jan\", \"Feb\"], [10, $feb], \"currency\"),\n  Toggle(\"Include tax\", $tax)\n])\n$feb = 12.5\n$tax = true\n```";
        let t = to_plain_text(src);
        assert!(t.contains("Jan: $10.00\nFeb: $12.50"), "{t}");
        assert!(t.contains("Include tax: On"), "{t}");
    }

    #[test]
    fn malformed_card_falls_back_and_never_leaks() {
        let t = to_plain_text("Before\n```openui\nthis is ((( not openui\n```\nAfter");
        assert_eq!(t, format!("Before\n\n{CARD_FALLBACK}\n\nAfter"));
        // Unclosed fence (cut-off reply).
        let lang = to_plain_text("A\n```openui-lang\nroot = Answer(\"T\", [Text(\"Body\")])\n```\nB");
        assert!(lang.contains("Body") && !lang.contains("```"), "{lang}");
        let t = to_plain_text("Hi\n```openui\nroot = Answer(\"T\", [Text(\"Body\")])");
        assert_eq!(t, "Hi\n\nT\n\nBody");
    }

    #[test]
    fn text_without_a_fence_is_unchanged() {
        let s = "Plain reply with ```js\ncode\n``` inside.";
        assert_eq!(to_plain_text(s), s);
        assert!(!has_fence(s));
    }

    #[test]
    fn spoken_filter_streams_text_and_holds_the_card() {
        let mut f = SpokenFilter::new();
        let mut said = String::new();
        for chunk in ["Sure. ", "Here you go.\n", "```open", "ui\nroot = Answer(\"Total\", ", "[Metric(\"Sum\", 2 + 3)])\n", "```\n", "Anything else?"] {
            said.push_str(&f.push(chunk));
        }
        assert!(said.starts_with("Sure. Here you go.\n"), "{said}");
        said.push_str(&f.finish());
        assert!(said.contains("Total"), "{said}");
        assert!(said.contains("Sum: 5"), "{said}");
        assert!(said.ends_with("Anything else?"), "{said}");
        assert!(!said.contains("```") && !said.contains("root ="), "{said}");
    }

    #[test]
    fn spoken_filter_does_not_hold_plain_text() {
        let mut f = SpokenFilter::new();
        assert_eq!(f.push("Hello there"), "Hello there");
        assert_eq!(f.push(" ``` code"), " ``` code");
        assert_eq!(f.finish(), "");
    }
}
