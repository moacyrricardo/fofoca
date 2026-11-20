//! Checking extracted notes against the transcript (spec 005). Pure functions: no model calls.
//!
//! Small local models keep names and numbers but invent details. Every extracted item carries the
//! transcript timestamps it came from; here we keep only what the cited lines actually support.

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

use crate::model::Segment;

// ------------------------------------------------------------------ model output (raw)

/// What the model returns (shape enforced by a JSON schema on llama-server).
#[derive(Deserialize, Debug, Default, Clone)]
pub struct RawNotes {
    #[serde(default)]
    pub summary: Vec<RawItem>,
    #[serde(default)]
    pub decisions: Vec<RawItem>,
    #[serde(default)]
    pub action_items: Vec<RawAction>,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct RawItem {
    pub text: String,
    #[serde(default)]
    pub refs: Vec<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct RawAction {
    pub text: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub refs: Vec<String>,
}

/// JSON schema for `RawNotes`, for llama-server's constrained decoding.
pub fn schema() -> serde_json::Value {
    let refs = serde_json::json!({ "type": "array", "items": { "type": "string", "pattern": "^[0-9]{2,3}:[0-9]{2}$" }, "minItems": 1, "maxItems": 4 });
    let item = serde_json::json!({ "type": "object", "properties": { "text": { "type": "string" }, "refs": refs }, "required": ["text", "refs"] });
    let nullable = serde_json::json!({ "type": ["string", "null"] });
    serde_json::json!({
        "type": "object",
        "properties": {
            "summary": { "type": "array", "items": item, "maxItems": 6 },
            "decisions": { "type": "array", "items": item, "maxItems": 8 },
            "action_items": { "type": "array", "maxItems": 10, "items": {
                "type": "object",
                "properties": { "text": { "type": "string" }, "owner": nullable, "due": nullable, "refs": refs },
                "required": ["text", "owner", "due", "refs"]
            }}
        },
        "required": ["summary", "decisions", "action_items"]
    })
}

// ------------------------------------------------------------------ validated notes

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Item {
    pub text: String,
    /// Start times (s) of the transcript lines it cites.
    pub refs: Vec<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Action {
    pub text: String,
    pub owner: Option<String>,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    pub refs: Vec<f64>,
}

/// An item the transcript doesn't support, with the reason (shown under "Couldn't verify").
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Flagged {
    pub section: String,
    pub text: String,
    pub reason: String,
    pub refs: Vec<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Notes {
    pub summary: Vec<Item>,
    pub decisions: Vec<Item>,
    pub action_items: Vec<Action>,
    pub flagged: Vec<Flagged>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub gen_tokens_per_s: f64,
}

/// Context the checks need besides the transcript.
pub struct Context<'a> {
    pub segments: &'a [Segment],
    /// Display names of voices in the meeting and invitees (owners must be one of them).
    pub people: &'a [String],
    pub started_at: DateTime<FixedOffset>,
}

pub fn validate(raw: &RawNotes, cx: &Context) -> Notes {
    let mut notes = Notes::default();
    for (section, items) in [("summary", &raw.summary), ("decisions", &raw.decisions)] {
        for it in items {
            match check(&it.text, &it.refs, cx) {
                Ok(refs) => {
                    let item = Item { text: it.text.trim().to_string(), refs };
                    if section == "summary" { notes.summary.push(item) } else { notes.decisions.push(item) }
                }
                Err((reason, refs)) => notes.flagged.push(Flagged { section: section.into(), text: it.text.trim().into(), reason, refs }),
            }
        }
    }
    for a in &raw.action_items {
        match check(&a.text, &a.refs, cx) {
            Ok(refs) => {
                let cited: Vec<&Segment> = cited_segments(&refs, cx.segments);
                notes.action_items.push(Action {
                    text: a.text.trim().to_string(),
                    owner: a.owner.as_deref().and_then(|o| match_person(o, cx.people)),
                    due: due_date(&cited, a.due.as_deref(), cx.started_at),
                    refs,
                });
            }
            Err((reason, refs)) => notes.flagged.push(Flagged { section: "action_items".into(), text: a.text.trim().into(), reason, refs }),
        }
    }
    // A small model often repeats a task as a summary point or "decision"; keep it once, as a task.
    let is_task = |t: &str| notes.action_items.iter().any(|a| same_text(&a.text, t));
    let (summary, decisions): (Vec<Item>, Vec<Item>) = (
        notes.summary.iter().filter(|i| !is_task(&i.text)).cloned().collect(),
        notes.decisions.iter().filter(|i| !is_task(&i.text)).cloned().collect(),
    );
    notes.summary = summary;
    notes.decisions = decisions;
    notes
}

fn same_text(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    norm(a) == norm(b)
}

/// Valid citations and every number in `text` present in a cited line or a line right next to
/// one (a statement often spans two lines and the model cites only the first); else why not.
fn check(text: &str, refs: &[String], cx: &Context) -> Result<Vec<f64>, (String, Vec<f64>)> {
    let mut valid: Vec<f64> = refs.iter().filter_map(|r| resolve_ref(r, cx.segments)).collect();
    valid.dedup();
    if valid.is_empty() {
        return Err(("no valid timestamp".into(), valid));
    }
    let cited_text: String = cx
        .segments
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let near = |j: usize| cx.segments.get(j).is_some_and(|s| valid.iter().any(|r| (s.start - r).abs() < 1e-9));
            near(*i) || near(i + 1) || (*i > 0 && near(i - 1))
        })
        .map(|(_, s)| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let supported = numbers_in(&cited_text);
    for n in numbers_in(text) {
        if !supported.iter().any(|s| (s - n).abs() < 1e-6) {
            return Err((format!("{} isn't said in the cited lines", fmt_num(n)), valid));
        }
    }
    Ok(valid)
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 { format!("{n:.0}") } else { format!("{n}") }
}

/// `"12:04"` → the start of the transcript line that begins within 2 s of it.
pub fn resolve_ref(r: &str, segments: &[Segment]) -> Option<f64> {
    let (m, s) = r.trim().trim_matches(['[', ']']).split_once(':')?;
    let t = m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?;
    segments
        .iter()
        .map(|seg| seg.start)
        .filter(|start| (start.floor() - t).abs() <= 2.0)
        .min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
}

fn cited_segments<'a>(refs: &[f64], segments: &'a [Segment]) -> Vec<&'a Segment> {
    segments.iter().filter(|s| refs.iter().any(|r| (s.start - r).abs() < 1e-9)).collect()
}

/// A name the model gave as owner, matched to a known person (full name, or first name when
/// unambiguous). Unknown names are dropped rather than guessed.
pub fn match_person(owner: &str, people: &[String]) -> Option<String> {
    let o = owner.trim().to_lowercase();
    if o.is_empty() {
        return None;
    }
    if let Some(p) = people.iter().find(|p| p.to_lowercase() == o) {
        return Some(p.clone());
    }
    let first = |s: &str| s.split_whitespace().next().unwrap_or("").to_lowercase();
    let hits: Vec<&String> = people.iter().filter(|p| first(p) == first(&o)).collect();
    (hits.len() == 1).then(|| hits[0].clone())
}

// ------------------------------------------------------------------ numbers

/// Numbers mentioned in a text: digits (`87.500`, `87,5`, `2026`), with `mil` / `milhão` /
/// `thousand` / `million` multipliers, and spelled-out Portuguese and English numbers
/// ("cento e vinte mil", "fifteen"). Ordinal suffixes and `R$`/`%` are ignored.
pub fn numbers_in(text: &str) -> Vec<f64> {
    let tokens = tokenize(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let Some(mut n) = parse_digits(&tokens[i]) {
            let mut j = i + 1;
            if let Some(m) = tokens.get(j).and_then(|t| multiplier(t)) {
                n *= m;
                j += 1;
            }
            out.push(n);
            i = j;
        } else if let Some((n, used)) = spelled(&tokens[i..]) {
            out.push(n);
            i += used;
        } else {
            i += 1;
        }
    }
    out
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '"' | '“' | '”' | ';' | '!' | '?' | '/'))
        .map(|t| t.trim_matches(|c: char| matches!(c, ',' | '.' | ':' | '-' | '\'' | '’')).to_lowercase())
        .map(|t| t.trim_start_matches("r$").trim_end_matches('%').trim_end_matches(['º', 'ª']).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// `87.500` / `87,500` (thousands) → 87500; `87,5` / `3.5` (decimals) → 87.5 / 3.5;
/// `87.500,00` / `1,234.5` (both) → the last separator is decimal when 1–2 digits follow it.
fn parse_digits(t: &str) -> Option<f64> {
    if !t.chars().next()?.is_ascii_digit() || !t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') {
        return None;
    }
    let groups: Vec<&str> = t.split(['.', ',']).collect();
    let (int, frac) = match groups.split_last() {
        Some((last, rest)) if !rest.is_empty() && last.len() != 3 => (rest.concat(), Some(*last)),
        _ => (groups.concat(), None), // no separator, or all groups of three = thousands
    };
    match frac {
        Some(f) => format!("{int}.{f}").parse().ok(),
        None => int.parse().ok(),
    }
}

fn multiplier(t: &str) -> Option<f64> {
    match t {
        "mil" | "thousand" | "k" => Some(1e3),
        "milhão" | "milhões" | "million" | "millions" | "mi" => Some(1e6),
        "bilhão" | "bilhões" | "billion" => Some(1e9),
        _ => None,
    }
}

fn unit(t: &str) -> Option<f64> {
    Some(match t {
        "zero" => 0.0,
        "um" | "uma" | "one" => 1.0,
        // ordinals that can't be mistaken for other words ("segundo" can be "according to")
        "primeiro" | "primeira" | "first" => 1.0,
        "terceiro" | "terceira" | "third" => 3.0,
        "dois" | "duas" | "two" => 2.0,
        "três" | "tres" | "three" => 3.0,
        "quatro" | "four" => 4.0,
        "cinco" | "five" => 5.0,
        "seis" | "six" => 6.0,
        "sete" | "seven" => 7.0,
        "oito" | "eight" => 8.0,
        "nove" | "nine" => 9.0,
        "dez" | "ten" => 10.0,
        "onze" | "eleven" => 11.0,
        "doze" | "twelve" => 12.0,
        "treze" | "thirteen" => 13.0,
        "catorze" | "quatorze" | "fourteen" => 14.0,
        "quinze" | "fifteen" => 15.0,
        "dezesseis" | "sixteen" => 16.0,
        "dezessete" | "seventeen" => 17.0,
        "dezoito" | "eighteen" => 18.0,
        "dezenove" | "nineteen" => 19.0,
        "vinte" | "twenty" => 20.0,
        "trinta" | "thirty" => 30.0,
        "quarenta" | "forty" => 40.0,
        "cinquenta" | "fifty" => 50.0,
        "sessenta" | "sixty" => 60.0,
        "setenta" | "seventy" => 70.0,
        "oitenta" | "eighty" => 80.0,
        "noventa" | "ninety" => 90.0,
        "cem" | "cento" | "hundred" => 100.0,
        "duzentos" | "duzentas" => 200.0,
        "trezentos" | "trezentas" => 300.0,
        "quatrocentos" | "quatrocentas" => 400.0,
        "quinhentos" | "quinhentas" => 500.0,
        "seiscentos" | "seiscentas" => 600.0,
        "setecentos" | "setecentas" => 700.0,
        "oitocentos" | "oitocentas" => 800.0,
        "novecentos" | "novecentas" => 900.0,
        _ => return None,
    })
}

/// A spelled-out number at the start of `tokens`: units joined by "e"/"and", with mil/milhão
/// groups. Returns the value and how many tokens it used. "um"/"uma"/"one" alone are articles, not
/// numbers.
fn spelled(tokens: &[String]) -> Option<(f64, usize)> {
    let mut total = 0.0;
    let mut group = 0.0;
    let mut used = 0;
    let mut last_was_number = false;
    let mut words = 0;
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].as_str();
        if let Some(u) = unit(t) {
            if t == "hundred" && group > 0.0 {
                group *= 100.0;
            } else {
                group += u;
            }
            last_was_number = true;
            words += 1;
            used = i + 1;
        } else if let Some(m) = multiplier(t).filter(|_| last_was_number || group > 0.0) {
            total += group.max(1.0) * m;
            group = 0.0;
            last_was_number = true;
            words += 1;
            used = i + 1;
        } else if (t == "e" || t == "and") && last_was_number && tokens.get(i + 1).is_some_and(|n| unit(n).is_some()) {
            last_was_number = false;
        } else {
            break;
        }
        i += 1;
    }
    if words == 0 || (words == 1 && matches!(tokens[0].as_str(), "um" | "uma" | "one")) {
        return None;
    }
    Some((total + group, used))
}

// ------------------------------------------------------------------ due dates

/// A due date only if the cited lines say when: an explicit "15 de novembro" / "november 15",
/// a weekday, or today / tomorrow / next week — resolved against the meeting date. The model's
/// own `due` is used only when the lines contain a date phrase we can't resolve ourselves.
pub fn due_date(cited: &[&Segment], model_due: Option<&str>, started: DateTime<FixedOffset>) -> Option<String> {
    let text = cited.iter().map(|s| s.text.to_lowercase()).collect::<Vec<_>>().join(" ");
    let day0 = started.date_naive();
    if let Some(d) = explicit_date(&text, day0.year()) {
        return Some(d.to_string());
    }
    let has = |w: &[&str]| w.iter().any(|x| text.contains(x));
    if has(&["amanhã", "amanha", "tomorrow"]) {
        return Some((day0 + Duration::days(1)).to_string());
    }
    if has(&["hoje", "today", "ainda hoje", "end of day"]) {
        return Some(day0.to_string());
    }
    for (names, wd) in WEEKDAYS {
        if names.iter().any(|n| text.contains(n)) {
            let ahead = (wd.num_days_from_monday() as i64 - day0.weekday().num_days_from_monday() as i64).rem_euclid(7);
            return Some((day0 + Duration::days(if ahead == 0 { 7 } else { ahead })).to_string());
        }
    }
    if has(&["semana que vem", "próxima semana", "proxima semana", "next week"]) {
        return model_due.and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map(|d| d.to_string());
    }
    None
}

const WEEKDAYS: [(&[&str], Weekday); 7] = [
    (&["segunda-feira", "segunda", "monday"], Weekday::Mon),
    (&["terça-feira", "terça", "terca", "tuesday"], Weekday::Tue),
    (&["quarta-feira", "quarta", "wednesday"], Weekday::Wed),
    (&["quinta-feira", "quinta", "thursday"], Weekday::Thu),
    (&["sexta-feira", "sexta", "friday"], Weekday::Fri),
    (&["sábado", "sabado", "saturday"], Weekday::Sat),
    (&["domingo", "sunday"], Weekday::Sun),
];

const MONTHS: [(&[&str], u32); 12] = [
    (&["janeiro", "january"], 1),
    (&["fevereiro", "february"], 2),
    (&["março", "marco", "march"], 3),
    (&["abril", "april"], 4),
    (&["maio", "may"], 5),
    (&["junho", "june"], 6),
    (&["julho", "july"], 7),
    (&["agosto", "august"], 8),
    (&["setembro", "september"], 9),
    (&["outubro", "october"], 10),
    (&["novembro", "november"], 11),
    (&["dezembro", "december"], 12),
];

/// "dia 15 de novembro" / "15 de novembro" / "november 15" (digits or spelled-out day).
fn explicit_date(text: &str, year: i32) -> Option<NaiveDate> {
    let tokens = tokenize(text);
    for (i, t) in tokens.iter().enumerate() {
        let Some(month) = MONTHS.iter().find(|(names, _)| names.contains(&t.as_str())).map(|(_, m)| *m) else { continue };
        // "15 de novembro": day before "de"
        let before = if i >= 2 && tokens[i - 1] == "de" { day_at(&tokens[..i - 1]) } else { None };
        // "november 15"
        let after = tokens.get(i + 1).and_then(|n| parse_digits(n));
        if let Some(day) = before.or(after).filter(|d| (1.0..=31.0).contains(d)) {
            return NaiveDate::from_ymd_opt(year, month, day as u32);
        }
    }
    None
}

/// The number ending at the last token(s) of `tokens` (digits, or a spelled-out number).
fn day_at(tokens: &[String]) -> Option<f64> {
    let last = tokens.last()?;
    if let Some(n) = parse_digits(last) {
        return Some(n);
    }
    // try spelled-out numbers of up to 3 words ending here ("vinte e um")
    (1..=3.min(tokens.len())).rev().find_map(|k| spelled(&tokens[tokens.len() - k..]).filter(|(_, used)| *used == k).map(|(n, _)| n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    fn seg(start: f64, text: &str) -> Segment {
        Segment { start, end: start + 5.0, text: text.into(), track: Track::Call, words: vec![], speaker: None }
    }

    #[test]
    fn reads_numbers_in_digits_and_words() {
        assert_eq!(numbers_in("executamos apenas 87.500"), vec![87500.0]);
        assert_eq!(numbers_in("orçamento de R$ 120 mil"), vec![120000.0]);
        assert_eq!(numbers_in("cento e vinte mil reais"), vec![120000.0]);
        assert_eq!(numbers_in("oitenta e sete mil e quinhentos"), vec![87500.0]);
        assert_eq!(numbers_in("87,5 mil"), vec![87500.0]);
        assert_eq!(numbers_in("terceiro trimestre de dois mil e vinte e seis"), vec![3.0, 2026.0]);
        assert_eq!(numbers_in("dia quinze de novembro"), vec![15.0]);
        assert_eq!(numbers_in("um cliente e uma reunião"), Vec::<f64>::new(), "articles aren't numbers");
        assert_eq!(numbers_in("growth of 3.5% and twenty one users"), vec![3.5, 21.0]);
        assert_eq!(numbers_in("1º lugar, 2026."), vec![1.0, 2026.0]);
        assert_eq!(numbers_in("terceiro trimestre"), vec![3.0]);
        assert_eq!(numbers_in("R$87.500,00 de R$120.000,00 e 1,234.5"), vec![87500.0, 120000.0, 1234.5]);
        assert_eq!(numbers_in("1.000.000 e 1,000"), vec![1000000.0, 1000.0]);
    }

    fn cx<'a>(segs: &'a [Segment], people: &'a [String]) -> Context<'a> {
        Context { segments: segs, people, started_at: DateTime::parse_from_rfc3339("2026-11-19T14:00:00-03:00").unwrap() }
    }

    #[test]
    fn keeps_supported_items_and_flags_invented_numbers() {
        let segs = [
            seg(0.0, "O orçamento previsto era de cento e vinte mil reais, porém executamos apenas oitenta e sete mil e quinhentos."),
            seg(16.0, "O cliente lá de Belo Horizonte sumiu do mapa."),
            seg(40.0, "Vou mandar o follow-up por e-mail ainda hoje, beleza?"),
        ];
        let people = vec!["Você".to_string(), "Fernanda Souza".to_string()];
        let raw = RawNotes {
            summary: vec![
                RawItem { text: "Orçamento de R$ 120 mil, executados R$ 87.500.".into(), refs: vec!["00:00".into()] },
                RawItem { text: "Diferença de R$ 26.250 no orçamento.".into(), refs: vec!["00:01".into()] },
                RawItem { text: "Cliente de BH sumiu.".into(), refs: vec!["00:17".into(), "09:59".into()] },
                RawItem { text: "Algo sem citação.".into(), refs: vec!["05:00".into()] },
            ],
            decisions: vec![],
            action_items: vec![RawAction { text: "Enviar o follow-up por e-mail".into(), owner: Some("Você".into()), due: Some("2026-12-01".into()), refs: vec!["00:40".into()] }],
        };
        let n = validate(&raw, &cx(&segs, &people));
        assert_eq!(n.summary.len(), 2, "{n:#?}");
        assert_eq!(n.summary[1].refs, vec![16.0], "bad ref dropped, close one resolved");
        assert_eq!(n.flagged.len(), 2);
        assert!(n.flagged[0].reason.contains("26250"), "{}", n.flagged[0].reason);
        assert_eq!(n.flagged[1].reason, "no valid timestamp");
        let a = &n.action_items[0];
        assert_eq!(a.owner.as_deref(), Some("Você"));
        assert_eq!(a.due.as_deref(), Some("2026-11-19"), "'ainda hoje' = meeting day, model's date ignored");
    }

    #[test]
    fn a_number_on_the_next_line_counts_and_tasks_arent_repeated() {
        let segs = [seg(34.0, "O orçamento previsto era de R$120 mil."), seg(39.0, "Bem, executamos apenas R$87.500."), seg(60.0, "Vou mandar o follow-up hoje.")];
        let people = vec![];
        let raw = RawNotes {
            summary: vec![
                RawItem { text: "Previsto R$120 mil, executado R$87.500.".into(), refs: vec!["00:34".into()] },
                RawItem { text: "Vou mandar o follow-up hoje".into(), refs: vec!["01:00".into()] },
            ],
            decisions: vec![],
            action_items: vec![RawAction { text: "Vou mandar o follow-up hoje.".into(), owner: None, due: None, refs: vec!["01:00".into()] }],
        };
        let n = validate(&raw, &cx(&segs, &people));
        assert!(n.flagged.is_empty(), "{:?}", n.flagged);
        assert_eq!(n.summary.len(), 1, "the repeated task is kept only as a task");
        assert_eq!(n.action_items.len(), 1);
    }

    #[test]
    fn owners_and_dates() {
        let people = vec!["Fernanda Souza".to_string(), "Rodrigo Lima".to_string(), "Rodrigo Alves".to_string()];
        assert_eq!(match_person("fernanda", &people).as_deref(), Some("Fernanda Souza"));
        assert_eq!(match_person("Rodrigo", &people), None, "ambiguous first name");
        assert_eq!(match_person("Rodrigo Lima", &people).as_deref(), Some("Rodrigo Lima"));
        assert_eq!(match_person("Carlos", &people), None);

        let start = DateTime::parse_from_rfc3339("2026-11-19T14:00:00-03:00").unwrap(); // a Thursday
        let s = |t: &str| seg(0.0, t);
        let due = |t: &str, m: Option<&str>| due_date(&[&s(t)], m, start);
        assert_eq!(due("o deploy foi na sexta-feira, dia quinze de novembro", None).as_deref(), Some("2026-11-15"));
        assert_eq!(due("entrego na sexta", None).as_deref(), Some("2026-11-20"));
        assert_eq!(due("faço isso amanhã", None).as_deref(), Some("2026-11-20"));
        assert_eq!(due("by november 30", None).as_deref(), Some("2026-11-30"));
        assert_eq!(due("vou ver isso", Some("2026-12-01")), None, "no date in the line: model's date dropped");
        assert_eq!(due("semana que vem eu vejo", Some("2026-11-26")).as_deref(), Some("2026-11-26"));
    }

    #[test]
    fn schema_matches_the_raw_types() {
        let s = schema();
        assert_eq!(s["required"], serde_json::json!(["summary", "decisions", "action_items"]));
        let example = serde_json::json!({ "summary": [{ "text": "x", "refs": ["01:02"] }], "decisions": [], "action_items": [{ "text": "y", "owner": null, "due": null, "refs": ["00:10"] }] });
        let raw: RawNotes = serde_json::from_value(example).unwrap();
        assert_eq!(raw.action_items[0].refs, vec!["00:10"]);
    }
}
