//! Calendars from private iCal links (spec 004).
//!
//! The user pastes a secret iCal address (Google: Settings → calendar → Integrate calendar →
//! "Secret address in iCal format") or a local `.ics` path. Links are credentials: they live in the
//! OS keyring, never in plain files or logs — `calendars.json` keeps only the host and a label.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "fofoca-calendar";

/// An event instance (recurrences expanded) in the requested window.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CalendarEvent {
    pub uid: String,
    pub title: String,
    /// RFC 3339, local time.
    pub start: String,
    pub end: String,
    pub all_day: bool,
    /// Meet / Zoom / Teams link if one was found.
    pub call_link: Option<String>,
    /// Display names (or e-mail local parts) of attendees.
    pub attendees: Vec<String>,
    /// Label of the calendar it came from.
    pub calendar: String,
    /// The calendar's default labels, applied when recording this event (spec 007).
    #[serde(default)]
    pub labels: Vec<String>,
}

// ---------------------------------------------------------------- parsing

struct Prop {
    name: String,
    params: HashMap<String, String>,
    value: String,
    raw: String,
}

/// Unfold RFC 5545 content lines (continuations start with a space or tab).
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = out.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn parse_prop(line: &str) -> Option<Prop> {
    // split at the first ':' outside double quotes
    let mut in_q = false;
    let idx = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            in_q = !in_q;
        }
        c == ':' && !in_q
    })?.0;
    let (head, value) = (&line[..idx], &line[idx + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some(Prop { name, params, value: value.to_string(), raw: line.to_string() })
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A DTSTART/DTEND value as a UTC instant. Returns (instant, all_day).
fn parse_time(p: &Prop) -> Option<(DateTime<Utc>, bool)> {
    let v = p.value.trim();
    if p.params.get("VALUE").map(|s| s == "DATE").unwrap_or(false) || v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        let local = Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest()?;
        return Some((local.with_timezone(&Utc), true));
    }
    let (naive_s, utc) = match v.strip_suffix('Z') {
        Some(s) => (s, true),
        None => (v, false),
    };
    let naive = NaiveDateTime::parse_from_str(naive_s, "%Y%m%dT%H%M%S").ok()?;
    if utc {
        return Some((Utc.from_utc_datetime(&naive), false));
    }
    if let Some(tz) = p.params.get("TZID").and_then(|t| t.parse::<chrono_tz::Tz>().ok()) {
        return Some((tz.from_local_datetime(&naive).earliest()?.with_timezone(&Utc), false));
    }
    Some((Local.from_local_datetime(&naive).earliest()?.with_timezone(&Utc), false))
}

#[derive(Default)]
struct RawEvent {
    props: Vec<Prop>,
}

impl RawEvent {
    fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a {
        self.props.iter().filter(move |p| p.name == name)
    }
}

/// Calendar name (`X-WR-CALNAME`) and events of an .ics document.
fn split_events(text: &str) -> (Option<String>, Vec<RawEvent>) {
    let mut name = None;
    let mut events = Vec::new();
    let mut cur: Option<RawEvent> = None;
    let mut depth_other = 0; // nested blocks inside VEVENT (VALARM)
    for line in unfold(text) {
        let Some(p) = parse_prop(&line) else { continue };
        match (p.name.as_str(), p.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => cur = Some(RawEvent::default()),
            ("END", "VEVENT") => {
                if let Some(e) = cur.take() {
                    events.push(e);
                }
            }
            ("BEGIN", _) if cur.is_some() => depth_other += 1,
            ("END", _) if cur.is_some() && depth_other > 0 => depth_other -= 1,
            ("X-WR-CALNAME", _) if cur.is_none() => name = Some(unescape(&p.value)),
            _ => {
                if let (Some(e), 0) = (cur.as_mut(), depth_other) {
                    e.props.push(p);
                }
            }
        }
    }
    (name, events)
}

/// Find a video-call link: Google's `X-GOOGLE-CONFERENCE`, else a Meet/Zoom/Teams URL in the
/// location or description.
fn call_link(e: &RawEvent) -> Option<String> {
    if let Some(p) = e.get("X-GOOGLE-CONFERENCE") {
        return Some(p.value.trim().to_string());
    }
    let hay = ["LOCATION", "DESCRIPTION", "URL"]
        .iter()
        .filter_map(|n| e.get(n).map(|p| unescape(&p.value)))
        .collect::<Vec<_>>()
        .join(" ");
    for marker in ["https://meet.google.com/", "https://teams.microsoft.com/l/meetup-join", "zoom.us/j/", "zoom.us/my/", "https://meet.jit.si/", "https://whereby.com/"] {
        if let Some(i) = hay.find(marker) {
            let start = hay[..i].rfind("https://").filter(|&s| !hay[s..i].contains(char::is_whitespace)).unwrap_or(i);
            let end = hay[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '<' | '>' | ')' | ']'))
                .map(|n| start + n)
                .unwrap_or(hay.len());
            return Some(hay[start..end].trim_end_matches(['.', ',']).to_string());
        }
    }
    None
}

fn attendees(e: &RawEvent) -> Vec<String> {
    e.all("ATTENDEE")
        .filter_map(|p| {
            if p.params.get("CUTYPE").map(|c| c == "RESOURCE" || c == "ROOM").unwrap_or(false) {
                return None;
            }
            p.params.get("CN").cloned().filter(|c| !c.contains('@')).or_else(|| {
                let mail = p.params.get("CN").cloned().unwrap_or_else(|| p.value.clone());
                mail.trim_start_matches("mailto:").split('@').next().map(str::to_string)
            })
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// Parse an .ics document and return event instances overlapping `[from, to)`, sorted by start.
pub fn events_between(text: &str, calendar: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<CalendarEvent> {
    let (_, raw) = split_events(text);

    // Instances overridden by a RECURRENCE-ID event are dropped from their series.
    let mut overridden: HashSet<(String, i64)> = HashSet::new();
    for e in &raw {
        if let (Some(uid), Some(rid)) = (e.get("UID"), e.get("RECURRENCE-ID").and_then(parse_time)) {
            overridden.insert((uid.value.clone(), rid.0.timestamp()));
        }
    }

    let mut out = Vec::new();
    for e in &raw {
        if e.get("STATUS").map(|s| s.value.eq_ignore_ascii_case("CANCELLED")).unwrap_or(false) {
            continue;
        }
        let Some((start, all_day)) = e.get("DTSTART").and_then(parse_time) else { continue };
        let duration = e
            .get("DTEND")
            .and_then(parse_time)
            .map(|(end, _)| end - start)
            .filter(|d| *d > Duration::zero())
            .unwrap_or(if all_day { Duration::days(1) } else { Duration::hours(1) });
        let uid = e.get("UID").map(|p| p.value.clone()).unwrap_or_default();
        let title = e.get("SUMMARY").map(|p| unescape(&p.value)).unwrap_or_else(|| "(no title)".into());
        let link = call_link(e);
        let people = attendees(e);

        let starts: Vec<DateTime<Utc>> = if e.get("RRULE").is_some() && e.get("RECURRENCE-ID").is_none() {
            expand(e, from - duration, to).unwrap_or_else(|| vec![start])
        } else {
            vec![start]
        };
        for s in starts {
            let end = s + duration;
            if end <= from || s >= to {
                continue;
            }
            if e.get("RECURRENCE-ID").is_none() && overridden.contains(&(uid.clone(), s.timestamp())) {
                continue;
            }
            out.push(CalendarEvent {
                uid: uid.clone(),
                title: title.clone(),
                start: s.with_timezone(&Local).to_rfc3339(),
                end: end.with_timezone(&Local).to_rfc3339(),
                all_day,
                call_link: link.clone(),
                attendees: people.clone(),
                calendar: calendar.to_string(),
                labels: Vec::new(),
            });
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start));
    out
}

/// Expand a recurring event with the `rrule` crate, feeding it the event's own DTSTART / RRULE /
/// RDATE / EXDATE lines.
fn expand(e: &RawEvent, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<Vec<DateTime<Utc>>> {
    let lines: Vec<String> = ["DTSTART", "RRULE", "RDATE", "EXDATE"]
        .iter()
        .flat_map(|n| e.all(n).map(|p| p.raw.clone()))
        .collect();
    let set: rrule::RRuleSet = lines.join("\n").parse().ok()?;
    let tz = rrule::Tz::UTC;
    let result = set
        .after(from.with_timezone(&tz))
        .before(to.with_timezone(&tz))
        .all(500);
    Some(result.dates.into_iter().map(|d| d.with_timezone(&Utc)).collect())
}

/// The calendar's own name (`X-WR-CALNAME`), if any.
pub fn calendar_name(text: &str) -> Option<String> {
    split_events(text).0
}

// ---------------------------------------------------------------- sources

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Secret URL — the URL itself lives in the keyring.
    Link,
    /// Local `.ics` file.
    File,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CalendarSource {
    pub id: String,
    pub kind: SourceKind,
    /// Calendar name from the feed, else the host / file name.
    pub label: String,
    /// Host for links (`calendar.google.com`) or the file path — never the secret.
    pub location: String,
    /// Labels given to meetings recorded from this calendar's events (spec 007).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_labels: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CalendarSettings {
    pub sources: Vec<CalendarSource>,
    /// Hide events without a call link from "Up next" (and from notifications / auto-record).
    pub only_with_call_link: bool,
    /// Desktop notification about a minute before a meeting starts.
    #[serde(default = "yes")]
    pub notify_before: bool,
    /// Start recording automatically when a meeting begins. Opt-in.
    #[serde(default)]
    pub auto_record: bool,
}

fn yes() -> bool {
    true
}

impl Default for CalendarSettings {
    fn default() -> Self {
        Self { sources: Vec::new(), only_with_call_link: true, notify_before: true, auto_record: false }
    }
}

/// Where secret links are kept. The real one is the OS keyring; tests use memory.
pub trait SecretStore: Send + Sync {
    fn set(&self, id: &str, secret: &str) -> Result<()>;
    fn get(&self, id: &str) -> Result<String>;
    fn delete(&self, id: &str) -> Result<()>;
}

/// Secret Service (GNOME Keyring / KWallet) on Linux.
pub struct Keyring;

impl SecretStore for Keyring {
    fn set(&self, id: &str, secret: &str) -> Result<()> {
        keyring::Entry::new(KEYRING_SERVICE, id)?.set_password(secret).context("saving the link in the system keyring")
    }
    fn get(&self, id: &str) -> Result<String> {
        keyring::Entry::new(KEYRING_SERVICE, id)?.get_password().context("reading the link from the system keyring")
    }
    fn delete(&self, id: &str) -> Result<()> {
        match keyring::Entry::new(KEYRING_SERVICE, id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("removing the link from the system keyring"),
        }
    }
}

pub struct Calendars<S: SecretStore> {
    path: PathBuf,
    secrets: S,
    http: reqwest::blocking::Client,
    /// Serializes read-modify-write of the settings file (commands run on parallel threads).
    write: std::sync::Mutex<()>,
}

impl<S: SecretStore> Calendars<S> {
    pub fn new(path: PathBuf, secrets: S) -> Result<Self> {
        Ok(Self {
            path,
            secrets,
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .user_agent("fo-foca")
                .build()?,
            write: Default::default(),
        })
    }

    pub fn settings(&self) -> CalendarSettings {
        std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, s: &CalendarSettings) -> Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&self.path, serde_json::to_vec_pretty(s)?)?;
        Ok(())
    }

    /// Change the options (not the sources) and save.
    pub fn update(&self, f: impl FnOnce(&mut CalendarSettings)) -> Result<CalendarSettings> {
        let _w = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.settings();
        f(&mut s);
        self.save(&s)?;
        Ok(s)
    }

    pub fn set_only_with_call_link(&self, v: bool) -> Result<CalendarSettings> {
        self.update(|s| s.only_with_call_link = v)
    }

    /// Add a secret link (https:// or webcal://) or a local .ics path. The feed is fetched and
    /// parsed first, so a wrong link is rejected before anything is stored.
    pub fn add(&self, input: &str) -> Result<CalendarSource> {
        let input = input.trim();
        let is_link = input.starts_with("https://") || input.starts_with("webcal://") || input.starts_with("http://");
        let path = PathBuf::from(input.trim_start_matches("file://"));
        if self.find_existing(input, is_link, &path).is_some() {
            bail!("that calendar is already added");
        }
        let (kind, location, text) = if is_link {
            let url = normalize_url(input);
            let host = url.split('/').nth(2).unwrap_or("calendar").to_string();
            let text = self.fetch(&url)?;
            (SourceKind::Link, host, text)
        } else {
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            (SourceKind::File, path.display().to_string(), text)
        };
        if !text.contains("BEGIN:VCALENDAR") {
            bail!("that doesn't look like an iCal calendar (no BEGIN:VCALENDAR)");
        }
        let label = calendar_name(&text).unwrap_or_else(|| match kind {
            SourceKind::Link => location.clone(),
            SourceKind::File => PathBuf::from(&location).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        });
        // The fetch above ran unlocked; re-check under the lock in case the same calendar was
        // added meanwhile, and keep ids unique even for adds in the same millisecond.
        let _w = self.write.lock().unwrap_or_else(|e| e.into_inner());
        if self.find_existing(input, is_link, &path).is_some() {
            bail!("that calendar is already added");
        }
        let mut s = self.settings();
        let mut n = Utc::now().timestamp_millis();
        while s.sources.iter().any(|x| x.id == format!("cal-{n}")) {
            n += 1;
        }
        let id = format!("cal-{n}");
        if kind == SourceKind::Link {
            self.secrets.set(&id, &normalize_url(input))?;
        }
        let source = CalendarSource { id, kind, label, location, default_labels: Vec::new() };
        s.sources.push(source.clone());
        self.save(&s)?;
        Ok(source)
    }

    /// The id of an already-added source with the same link (compared via the keyring) or file.
    fn find_existing(&self, input: &str, is_link: bool, path: &std::path::Path) -> Option<String> {
        let url = normalize_url(input);
        self.settings().sources.into_iter().find_map(|x| {
            let same = match x.kind {
                SourceKind::Link => is_link && self.secrets.get(&x.id).is_ok_and(|u| u == url),
                SourceKind::File => !is_link && std::path::Path::new(&x.location) == path,
            };
            same.then_some(x.id)
        })
    }

    /// Set the labels applied to meetings recorded from this calendar's events.
    pub fn set_default_labels<L: AsRef<str>>(&self, id: &str, labels: &[L]) -> Result<CalendarSettings> {
        let labels = crate::labels::normalize_all(labels);
        let _w = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.settings();
        let src = s.sources.iter_mut().find(|x| x.id == id).with_context(|| format!("no calendar with id {id}"))?;
        src.default_labels = labels;
        self.save(&s)?;
        Ok(s)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let _w = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.settings();
        if let Some(src) = s.sources.iter().find(|x| x.id == id) {
            if src.kind == SourceKind::Link {
                self.secrets.delete(id)?;
            }
        }
        s.sources.retain(|x| x.id != id);
        self.save(&s)
    }

    /// Upcoming event instances from all calendars. Per-calendar failures are returned as
    /// messages (with the label, never the link) so one broken feed doesn't hide the others.
    pub fn upcoming(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> (Vec<CalendarEvent>, Vec<String>) {
        let settings = self.settings();
        let mut events = Vec::new();
        let mut errors = Vec::new();
        for src in &settings.sources {
            let text = match src.kind {
                SourceKind::Link => self.secrets.get(&src.id).and_then(|url| self.fetch(&url)),
                SourceKind::File => std::fs::read_to_string(&src.location).map_err(Into::into),
            };
            match text {
                Ok(t) => events.extend(events_between(&t, &src.label, from, to).into_iter().map(|mut e| {
                    e.labels = src.default_labels.clone();
                    e
                })),
                Err(e) => errors.push(format!("{}: {}", src.label, redact(&format!("{e:#}")))),
            }
        }
        if settings.only_with_call_link {
            events.retain(|e| e.call_link.is_some());
        }
        events.sort_by(|a, b| a.start.cmp(&b.start));
        (events, errors)
    }

    fn fetch(&self, url: &str) -> Result<String> {
        let r = self.http.get(url).send().map_err(|e| anyhow::anyhow!("could not reach the calendar ({})", redact(&e.to_string())))?;
        if !r.status().is_success() {
            bail!("the calendar answered {} — the link may have been reset", r.status());
        }
        Ok(r.text()?)
    }
}

fn normalize_url(u: &str) -> String {
    match u.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => u.to_string(),
    }
}

/// Error texts from the HTTP client can contain the URL; strip anything that looks like one.
fn redact(s: &str) -> String {
    s.split_whitespace()
        .map(|w| if w.contains("://") || w.contains("/ical/") { "<link>" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Shaped like a Google Calendar export: VTIMEZONE, weekly RRULE with an EXDATE and one
    // moved instance, Meet link, attendees, a cancelled event and an all-day event.
    const ICS: &str = "BEGIN:VCALENDAR\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nVERSION:2.0\r\n\
X-WR-CALNAME:Trabalho\r\nX-WR-TIMEZONE:America/Sao_Paulo\r\n\
BEGIN:VTIMEZONE\r\nTZID:America/Sao_Paulo\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:-0300\r\nTZOFFSETTO:-0300\r\nDTSTART:19700101T000000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nDTSTART;TZID=America/Sao_Paulo:20261104T160000\r\nDTEND;TZID=America/Sao_Paulo:20261104T163000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=WE\r\nEXDATE;TZID=America/Sao_Paulo:20261111T160000\r\nUID:sync@google.com\r\n\
SUMMARY:Sync semanal \\, produto\r\nX-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r\n\
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Rodrigo Lima;X-NUM-GUESTS=0:mailto:rodrigo@example.com\r\n\
ATTENDEE;CUTYPE=INDIVIDUAL;CN=fernanda@example.com:mailto:fernanda@example.com\r\n\
ATTENDEE;CUTYPE=RESOURCE;CN=Sala 3:mailto:sala3@resource.example.com\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:This is an event reminder\r\nTRIGGER:-P0DT0H10M0S\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nDTSTART;TZID=America/Sao_Paulo:20261125T170000\r\nDTEND;TZID=America/Sao_Paulo:20261125T173000\r\n\
RECURRENCE-ID;TZID=America/Sao_Paulo:20261125T160000\r\nUID:sync@google.com\r\nSUMMARY:Sync semanal (moved)\r\n\
X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nDTSTART:20261120T130000Z\r\nDTEND:20261120T140000Z\r\nUID:kickoff@x\r\nSUMMARY:Kickoff cliente BH\r\n\
DESCRIPTION:Entrar com Zoom: https://us02web.zoom.us/j/123456789?pwd=abc. Obrigado\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nDTSTART:20261121T130000Z\r\nDTEND:20261121T140000Z\r\nUID:gone@x\r\nSTATUS:CANCELLED\r\nSUMMARY:Cancelada\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261122\r\nDTEND;VALUE=DATE:20261123\r\nUID:allday@x\r\nSUMMARY:Feriado\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn expands_recurrence_with_exdate_and_override() {
        let ev = events_between(ICS, "Trabalho", utc("2026-11-01T00:00:00Z"), utc("2026-12-01T00:00:00Z"));
        let sync: Vec<_> = ev.iter().filter(|e| e.uid == "sync@google.com").collect();
        // Nov 4, (11 excluded), 18, 25 moved to 17:00
        assert_eq!(sync.len(), 3, "{sync:#?}");
        let starts: Vec<DateTime<Utc>> = sync.iter().map(|e| utc(&e.start)).collect();
        assert!(starts.contains(&utc("2026-11-04T19:00:00Z")));
        assert!(starts.contains(&utc("2026-11-18T19:00:00Z")));
        assert!(starts.contains(&utc("2026-11-25T20:00:00Z")));
        assert!(!starts.contains(&utc("2026-11-11T19:00:00Z")));
        assert!(!starts.contains(&utc("2026-11-25T19:00:00Z")));
        let first = sync.iter().find(|e| utc(&e.start) == utc("2026-11-04T19:00:00Z")).unwrap();
        assert_eq!(first.title, "Sync semanal , produto");
        assert_eq!(utc(&first.end) - utc(&first.start), Duration::minutes(30));
        assert_eq!(first.call_link.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert_eq!(first.attendees, vec!["Rodrigo Lima".to_string(), "fernanda".to_string()]);
    }

    #[test]
    fn finds_links_skips_cancelled_handles_all_day() {
        let ev = events_between(ICS, "Trabalho", utc("2026-11-19T00:00:00Z"), utc("2026-11-24T00:00:00Z"));
        let titles: Vec<&str> = ev.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"Kickoff cliente BH"));
        assert!(!titles.contains(&"Cancelada"));
        let kickoff = ev.iter().find(|e| e.uid == "kickoff@x").unwrap();
        assert_eq!(kickoff.call_link.as_deref(), Some("https://us02web.zoom.us/j/123456789?pwd=abc"));
        let feriado = ev.iter().find(|e| e.uid == "allday@x").unwrap();
        assert!(feriado.all_day);
        assert_eq!(calendar_name(ICS).as_deref(), Some("Trabalho"));
    }

    #[test]
    fn unfolds_continuation_lines() {
        assert_eq!(unfold("SUMMARY:Long\r\n  title\r\nUID:1"), vec!["SUMMARY:Long title", "UID:1"]);
    }

    #[derive(Default)]
    struct Mem(Mutex<HashMap<String, String>>);
    impl SecretStore for Mem {
        fn set(&self, id: &str, s: &str) -> Result<()> {
            self.0.lock().unwrap().insert(id.into(), s.into());
            Ok(())
        }
        fn get(&self, id: &str) -> Result<String> {
            self.0.lock().unwrap().get(id).cloned().context("missing")
        }
        fn delete(&self, id: &str) -> Result<()> {
            self.0.lock().unwrap().remove(id);
            Ok(())
        }
    }

    #[test]
    fn file_sources_roundtrip_and_filter() {
        let dir = std::env::temp_dir().join(format!("fofoca-cal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ics = dir.join("work.ics");
        std::fs::write(&ics, ICS).unwrap();
        let cals = Calendars::new(dir.join("calendars.json"), Mem::default()).unwrap();
        let src = cals.add(ics.to_str().unwrap()).unwrap();
        assert_eq!(src.label, "Trabalho");
        assert_eq!(src.kind, SourceKind::File);
        let (ev, errs) = cals.upcoming(utc("2026-11-19T00:00:00Z"), utc("2026-11-24T00:00:00Z"));
        assert!(errs.is_empty());
        assert!(ev.iter().all(|e| e.call_link.is_some()), "call-link filter is on by default");
        cals.set_only_with_call_link(false).unwrap();
        let (ev2, _) = cals.upcoming(utc("2026-11-19T00:00:00Z"), utc("2026-11-24T00:00:00Z"));
        assert!(ev2.len() > ev.len());
        assert!(cals.add(dir.join("missing.ics").to_str().unwrap()).is_err());
        let s = cals.update(|s| s.auto_record = true).unwrap();
        assert!(s.auto_record && s.notify_before && !s.only_with_call_link);
        cals.remove(&src.id).unwrap();
        assert!(cals.settings().sources.is_empty());
        assert!(cals.settings().auto_record, "options survive removing a source");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn several_calendars_merge_and_reject_duplicates() {
        let dir = std::env::temp_dir().join(format!("fofoca-cal-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.ics"), dir.join("b.ics"));
        std::fs::write(&a, ICS).unwrap();
        std::fs::write(&b, ICS.replace("X-WR-CALNAME:Trabalho", "X-WR-CALNAME:Pessoal")).unwrap();
        let cals = Calendars::new(dir.join("calendars.json"), Mem::default()).unwrap();
        cals.set_only_with_call_link(false).unwrap();
        let sa = cals.add(a.to_str().unwrap()).unwrap();
        let sb = cals.add(b.to_str().unwrap()).unwrap();
        assert_ne!(sa.id, sb.id);
        assert!(cals.add(a.to_str().unwrap()).is_err(), "same file twice is rejected");
        assert_eq!(cals.settings().sources.len(), 2);
        let (ev, errs) = cals.upcoming(utc("2026-11-19T00:00:00Z"), utc("2026-11-24T00:00:00Z"));
        assert!(errs.is_empty());
        let labels: HashSet<_> = ev.iter().map(|e| e.calendar.as_str()).collect();
        assert_eq!(labels, HashSet::from(["Trabalho", "Pessoal"]));
        cals.set_default_labels(&sb.id, &["Pessoal", "família"]).unwrap();
        assert!(cals.set_default_labels("nope", &["x"]).is_err());
        let (ev, _) = cals.upcoming(utc("2026-11-19T00:00:00Z"), utc("2026-11-24T00:00:00Z"));
        assert!(ev.iter().filter(|e| e.calendar == "Pessoal").all(|e| e.labels == ["pessoal", "família"]));
        assert!(ev.iter().filter(|e| e.calendar == "Trabalho").all(|e| e.labels.is_empty()));
        cals.remove(&sa.id).unwrap();
        assert_eq!(cals.settings().sources.iter().map(|s| &s.id).collect::<Vec<_>>(), vec![&sb.id]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn redacts_links_from_errors() {
        assert_eq!(redact("error sending request for url (https://calendar.google.com/x/private-abc/basic.ics)"), "error sending request for url <link>");
    }
}
