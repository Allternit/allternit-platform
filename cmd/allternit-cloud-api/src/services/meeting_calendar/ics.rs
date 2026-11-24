//! A small iCalendar (RFC 5545) reader for pasted calendar feeds: Outlook
//! "Publish calendar", iCloud public calendars, Google's secret ICS address,
//! any https `.ics`. It reads only what Meetings needs: VEVENTs with a
//! meeting link, their times (TZID via IANA names, the feed's VTIMEZONE,
//! or the Windows zone names Outlook writes), attendees for the 1:1 rule,
//! recurrences inside the sync window (RRULE, RDATE, EXDATE and
//! RECURRENCE-ID overrides), and cancellations. All-day events are ignored.
//!
//! No crate in the workspace parses ICS, and the recurrence rules feeds use
//! in practice are a small subset, so this is hand-written. Unsupported rule
//! parts (BYHOUR, BYWEEKNO, BYYEARDAY, sub-daily frequencies) keep only the
//! first occurrence instead of guessing.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};

use super::{Attendee, CalendarEvent};
use crate::routes::meeting_links;

/// Most components a feed may hold before the rest are ignored.
const MAX_COMPONENTS: usize = 50_000;
/// Longest a recurrence is walked day by day (COUNT rules start at DTSTART).
const MAX_RECURRENCE_DAYS: i64 = 40 * 366;

// ---------------------------------------------------------------------------
// Content lines and components
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Default)]
struct Component {
    name: String,
    props: Vec<Prop>,
    children: Vec<Component>,
}

impl Component {
    fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
    fn props<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a {
        self.props.iter().filter(move |p| p.name == name)
    }
    fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(|p| unescape(&p.value)).filter(|s| !s.trim().is_empty())
    }
}

/// Undo line folding (CRLF or LF followed by one space or tab).
fn unfold(body: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in body.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// `NAME;P1=a;P2="b:c":value`. Parameter values may be quoted; the first
/// colon outside quotes starts the value.
fn parse_line(line: &str) -> Option<Prop> {
    let mut in_quotes = false;
    let mut split = None;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let split = split?;
    let (head, value) = (&line[..split], &line[split + 1..]);
    let mut parts = Vec::new();
    let mut current = String::new();
    in_quotes = false;
    for ch in head.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => parts.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    parts.push(current);
    let mut parts = parts.into_iter();
    let name = parts.next()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_ascii_uppercase(), v.trim().to_string()))
        })
        .collect();
    Some(Prop { name, params, value: value.to_string() })
}

fn parse_components(body: &str) -> Option<Component> {
    let mut stack: Vec<Component> = Vec::new();
    let mut root: Option<Component> = None;
    let mut count = 0usize;
    for line in unfold(body) {
        let Some(prop) = parse_line(&line) else { continue };
        match prop.name.as_str() {
            "BEGIN" => {
                count += 1;
                if count > MAX_COMPONENTS {
                    break;
                }
                stack.push(Component { name: prop.value.trim().to_ascii_uppercase(), ..Default::default() });
            }
            "END" => {
                let Some(done) = stack.pop() else { continue };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None if done.name == "VCALENDAR" && root.is_none() => root = Some(done),
                    None => {}
                }
            }
            _ => {
                if let Some(top) = stack.last_mut() {
                    top.props.push(prop);
                }
            }
        }
    }
    // A feed cut off before END:VCALENDAR still yields what was read.
    while let Some(done) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(done),
            None if done.name == "VCALENDAR" && root.is_none() => root = Some(done),
            None => {}
        }
    }
    root
}

/// TEXT unescaping: `\n`, `\,`, `\;`, `\\`.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// True when the body is an iCalendar object (the feed check on connect).
pub fn is_vcalendar(body: &str) -> bool {
    let start = body.trim_start_matches('\u{feff}').trim_start();
    start.len() >= 15 && start[..15].eq_ignore_ascii_case("BEGIN:VCALENDAR") && parse_components(start).is_some()
}

// ---------------------------------------------------------------------------
// Times and zones
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum IcsTime {
    Date(NaiveDate),
    Utc(DateTime<Utc>),
    Local(NaiveDateTime),
}

fn parse_naive(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M").ok())
}

fn parse_time_value(value: &str, is_date: bool) -> Option<IcsTime> {
    let value = value.trim();
    if is_date || (value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit())) {
        return NaiveDate::parse_from_str(value, "%Y%m%d").ok().map(IcsTime::Date);
    }
    match value.strip_suffix('Z').or_else(|| value.strip_suffix('z')) {
        Some(utc) => parse_naive(utc).map(|n| IcsTime::Utc(Utc.from_utc_datetime(&n))),
        None => parse_naive(value).map(IcsTime::Local),
    }
}

/// A property's time(s) and its TZID (EXDATE/RDATE may list several).
fn prop_times(prop: &Prop) -> (Vec<IcsTime>, Option<String>) {
    let is_date = prop.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
    let times = prop.value.split(',').filter_map(|v| parse_time_value(v, is_date)).collect();
    (times, prop.param("TZID").map(|t| t.trim_matches('"').to_string()))
}

#[derive(Debug, Clone, Copy)]
enum YearlyDay {
    /// `BYDAY=2SU` / `-1SU` in BYMONTH.
    Nth(i32, Weekday),
    /// Old style `BYMONTHDAY=8,9,...;BYDAY=SU`: first weekday on or after the day.
    OnOrAfter(u32, Weekday),
}

#[derive(Debug, Clone)]
struct Observance {
    start: NaiveDateTime,
    offset_from: i32,
    offset_to: i32,
    rule: Option<(u32, YearlyDay)>,
}

#[derive(Debug, Clone, Default)]
struct VTimezone {
    observances: Vec<Observance>,
}

fn parse_offset(value: &str) -> Option<i32> {
    let v = value.trim();
    let (sign, rest) = match v.as_bytes().first()? {
        b'+' => (1, &v[1..]),
        b'-' => (-1, &v[1..]),
        _ => (1, v),
    };
    if rest.len() < 4 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i32 = rest[0..2].parse().ok()?;
    let m: i32 = rest[2..4].parse().ok()?;
    let s: i32 = if rest.len() >= 6 { rest[4..6].parse().ok()? } else { 0 };
    Some(sign * (h * 3600 + m * 60 + s))
}

fn parse_weekday(code: &str) -> Option<Weekday> {
    Some(match code.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// `2SU` -> (Some(2), Sun); `SU` -> (None, Sun); `-1FR` -> (Some(-1), Fri).
fn parse_byday(item: &str) -> Option<(Option<i32>, Weekday)> {
    let item = item.trim();
    if item.len() < 2 {
        return None;
    }
    let (num, code) = item.split_at(item.len() - 2);
    let weekday = parse_weekday(code)?;
    let ordinal = match num.trim_start_matches('+') {
        "" => None,
        n => Some(n.parse::<i32>().ok().filter(|n| *n != 0)?),
    };
    Some((ordinal, weekday))
}

impl VTimezone {
    fn parse(component: &Component) -> Self {
        let observances = component
            .children
            .iter()
            .filter(|c| c.name == "STANDARD" || c.name == "DAYLIGHT")
            .filter_map(|c| {
                let start = c.prop("DTSTART").and_then(|p| parse_naive(p.value.trim()))?;
                let offset_to = c.prop("TZOFFSETTO").and_then(|p| parse_offset(&p.value))?;
                let offset_from = c.prop("TZOFFSETFROM").and_then(|p| parse_offset(&p.value)).unwrap_or(offset_to);
                let rule = c.prop("RRULE").and_then(|p| yearly_rule(&p.value));
                Some(Observance { start, offset_from, offset_to, rule })
            })
            .collect();
        VTimezone { observances }
    }

    /// UTC offset (seconds) in force at local wall time `local`.
    fn offset_at(&self, local: NaiveDateTime) -> Option<i32> {
        let mut best: Option<(NaiveDateTime, i32)> = None;
        for obs in &self.observances {
            let mut candidates = Vec::new();
            match obs.rule {
                Some((month, day)) => {
                    for year in [local.year() - 1, local.year()] {
                        if year < obs.start.year() {
                            continue;
                        }
                        if let Some(date) = yearly_date(year, month, day) {
                            candidates.push(date.and_time(obs.start.time()));
                        }
                    }
                }
                None => candidates.push(obs.start),
            }
            for at in candidates {
                // Transition wall times are in the offset before the change.
                let at_utc = at - Duration::seconds(obs.offset_from as i64);
                let local_utc = local - Duration::seconds(obs.offset_from as i64);
                if at_utc <= local_utc && best.is_none_or(|(b, _)| at > b) {
                    best = Some((at, obs.offset_to));
                }
            }
        }
        best.map(|(_, offset)| offset).or_else(|| self.observances.first().map(|o| o.offset_to))
    }
}

fn yearly_rule(rrule: &str) -> Option<(u32, YearlyDay)> {
    let parts: HashMap<String, String> = rrule
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().to_string()))
        .collect();
    if parts.get("FREQ").map(String::as_str) != Some("YEARLY") {
        return None;
    }
    let month: u32 = parts.get("BYMONTH")?.split(',').next()?.parse().ok()?;
    let (ordinal, weekday) = parse_byday(parts.get("BYDAY")?.split(',').next()?)?;
    match (ordinal, parts.get("BYMONTHDAY")) {
        (Some(n), _) => Some((month, YearlyDay::Nth(n, weekday))),
        (None, Some(days)) => {
            let first: u32 = days.split(',').filter_map(|d| d.trim().parse().ok()).min()?;
            Some((month, YearlyDay::OnOrAfter(first, weekday)))
        }
        (None, None) => None,
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

fn nth_weekday(year: i32, month: u32, n: i32, weekday: Weekday) -> Option<NaiveDate> {
    if n > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let shift = (7 + weekday.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64) % 7;
        let date = first + Duration::days(shift + 7 * (n as i64 - 1));
        (date.month() == month).then_some(date)
    } else {
        let last = NaiveDate::from_ymd_opt(year, month, days_in_month(year, month))?;
        let shift = (7 + last.weekday().num_days_from_monday() as i64 - weekday.num_days_from_monday() as i64) % 7;
        let date = last - Duration::days(shift + 7 * (-n as i64 - 1));
        (date.month() == month).then_some(date)
    }
}

fn yearly_date(year: i32, month: u32, day: YearlyDay) -> Option<NaiveDate> {
    match day {
        YearlyDay::Nth(n, weekday) => nth_weekday(year, month, n, weekday),
        YearlyDay::OnOrAfter(from, weekday) => {
            let start = NaiveDate::from_ymd_opt(year, month, from)?;
            let shift = (7 + weekday.num_days_from_monday() as i64 - start.weekday().num_days_from_monday() as i64) % 7;
            Some(start + Duration::days(shift))
        }
    }
}

/// Windows time zone names Outlook writes as TZID, mapped to IANA (CLDR
/// windowsZones, territory 001). Covers the zones nearly every feed uses.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time", "America/Denver"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time", "America/Chicago"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Canada Central Standard Time", "America/Regina"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time", "America/New_York"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("US Eastern Standard Time", "America/Indianapolis"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("Argentina Standard Time", "America/Buenos_Aires"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Greenland Standard Time", "America/Godthab"),
    ("UTC", "Etc/UTC"),
    ("Coordinated Universal Time", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("India Standard Time", "Asia/Calcutta"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Katmandu"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
];

#[derive(Debug, Clone)]
enum Zone {
    Iana(chrono_tz::Tz),
    Feed(VTimezone),
    Fixed(i32),
}

impl Zone {
    fn to_utc(&self, local: NaiveDateTime) -> Option<DateTime<Utc>> {
        match self {
            Zone::Iana(tz) => tz
                .from_local_datetime(&local)
                .earliest()
                // In a spring-forward gap the wall time doesn't exist: use
                // the time an hour later, as calendar apps do.
                .or_else(|| tz.from_local_datetime(&(local + Duration::hours(1))).earliest())
                .map(|t| t.with_timezone(&Utc)),
            Zone::Feed(vtz) => {
                let offset = vtz.offset_at(local)?;
                Some(Utc.from_utc_datetime(&(local - Duration::seconds(offset as i64))))
            }
            Zone::Fixed(offset) => FixedOffset::east_opt(*offset)?
                .from_local_datetime(&local)
                .single()
                .map(|t| t.with_timezone(&Utc)),
        }
    }

    /// Local wall time of a UTC instant (for EXDATE dates and UNTIL).
    fn to_local(&self, at: DateTime<Utc>) -> NaiveDateTime {
        match self {
            Zone::Iana(tz) => at.with_timezone(tz).naive_local(),
            Zone::Feed(vtz) => {
                let guess = at.naive_utc() + Duration::seconds(vtz.observances.first().map_or(0, |o| o.offset_to) as i64);
                at.naive_utc() + Duration::seconds(vtz.offset_at(guess).unwrap_or(0) as i64)
            }
            Zone::Fixed(offset) => at.naive_utc() + Duration::seconds(*offset as i64),
        }
    }
}

struct Zones {
    feed: HashMap<String, VTimezone>,
    default: Zone,
}

impl Zones {
    fn new(calendar: &Component) -> Self {
        let feed: HashMap<String, VTimezone> = calendar
            .children
            .iter()
            .filter(|c| c.name == "VTIMEZONE")
            .filter_map(|c| Some((c.prop("TZID")?.value.trim().trim_matches('"').to_string(), VTimezone::parse(c))))
            .collect();
        let default = calendar
            .prop("X-WR-TIMEZONE")
            .and_then(|p| iana(p.value.trim()))
            .map(Zone::Iana)
            .unwrap_or(Zone::Iana(chrono_tz::UTC));
        Zones { feed, default }
    }

    /// TZID -> zone: an IANA name (also inside a `/mozilla.org/.../Europe/Paris`
    /// style path), the feed's own VTIMEZONE, a Windows zone name, a
    /// `(UTC-08:00) ...` label; else the calendar default.
    fn zone(&self, tzid: Option<&str>) -> Zone {
        let Some(id) = tzid.map(|t| t.trim().trim_matches('"')).filter(|t| !t.is_empty()) else {
            return self.default.clone();
        };
        if let Some(tz) = iana(id) {
            return Zone::Iana(tz);
        }
        if let Some(vtz) = self.feed.get(id).filter(|v| !v.observances.is_empty()) {
            // A feed zone named after a Windows zone resolves to IANA rules,
            // which also know historical changes.
            if let Some(tz) = windows_zone(id) {
                return Zone::Iana(tz);
            }
            return Zone::Feed(vtz.clone());
        }
        if let Some(tz) = windows_zone(id) {
            return Zone::Iana(tz);
        }
        if let Some(offset) = utc_label_offset(id) {
            return Zone::Fixed(offset);
        }
        tracing::debug!("meeting calendar: unknown TZID {id:?}, using the calendar default");
        self.default.clone()
    }

    fn resolve(&self, time: IcsTime, tzid: Option<&str>) -> Option<DateTime<Utc>> {
        match time {
            IcsTime::Utc(t) => Some(t),
            IcsTime::Local(local) => self.zone(tzid).to_utc(local),
            IcsTime::Date(_) => None,
        }
    }
}

fn iana(name: &str) -> Option<chrono_tz::Tz> {
    if let Ok(tz) = name.parse::<chrono_tz::Tz>() {
        return Some(tz);
    }
    // `/mozilla.org/20050126_1/America/New_York`, `/softwarestudio.org/.../Europe/Paris`
    name.match_indices('/').find_map(|(i, _)| name[i + 1..].parse::<chrono_tz::Tz>().ok())
}

fn windows_zone(name: &str) -> Option<chrono_tz::Tz> {
    WINDOWS_ZONES
        .iter()
        .find(|(windows, _)| windows.eq_ignore_ascii_case(name))
        .and_then(|(_, iana_name)| iana_name.parse().ok())
}

/// `(UTC-08:00) Pacific Time (US & Canada)` or `GMT+0530` -> fixed offset.
fn utc_label_offset(label: &str) -> Option<i32> {
    let upper = label.to_ascii_uppercase();
    let at = upper.find("UTC").or_else(|| upper.find("GMT"))?;
    let rest: String = upper[at + 3..].chars().take_while(|c| matches!(c, '+' | '-' | ':' | '0'..='9')).collect();
    if rest.is_empty() {
        return Some(0);
    }
    parse_offset(&rest.replace(':', ""))
}

// ---------------------------------------------------------------------------
// Recurrence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone)]
struct RRule {
    freq: Freq,
    interval: i64,
    count: Option<u32>,
    until: Option<IcsTime>,
    by_day: Vec<(Option<i32>, Weekday)>,
    by_month_day: Vec<i32>,
    by_month: Vec<u32>,
    by_set_pos: Vec<i32>,
    wkst: Weekday,
}

/// `None` when the rule uses parts this reader doesn't expand.
fn parse_rrule(value: &str) -> Option<RRule> {
    let mut rule = RRule {
        freq: Freq::Daily,
        interval: 1,
        count: None,
        until: None,
        by_day: Vec::new(),
        by_month_day: Vec::new(),
        by_month: Vec::new(),
        by_set_pos: Vec::new(),
        wkst: Weekday::Mon,
    };
    let mut freq = None;
    for part in value.split(';') {
        let Some((key, val)) = part.split_once('=') else { continue };
        let val = val.trim();
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => {
                freq = Some(match val.to_ascii_uppercase().as_str() {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return None,
                })
            }
            "INTERVAL" => rule.interval = val.parse::<i64>().ok().filter(|n| *n >= 1)?,
            "COUNT" => rule.count = Some(val.parse().ok()?),
            "UNTIL" => rule.until = Some(parse_time_value(val, false)?),
            "BYDAY" => rule.by_day = val.split(',').map(parse_byday).collect::<Option<_>>()?,
            "BYMONTHDAY" => {
                rule.by_month_day = val
                    .split(',')
                    .map(|d| d.trim().parse::<i32>().ok().filter(|d| *d != 0 && d.abs() <= 31))
                    .collect::<Option<_>>()?
            }
            "BYMONTH" => {
                rule.by_month = val
                    .split(',')
                    .map(|m| m.trim().parse::<u32>().ok().filter(|m| (1..=12).contains(m)))
                    .collect::<Option<_>>()?
            }
            "BYSETPOS" => {
                rule.by_set_pos =
                    val.split(',').map(|p| p.trim().parse::<i32>().ok().filter(|p| *p != 0)).collect::<Option<_>>()?
            }
            "WKST" => rule.wkst = parse_weekday(val)?,
            // BYHOUR, BYMINUTE, BYSECOND, BYWEEKNO, BYYEARDAY, RSCALE...
            _ => return None,
        }
    }
    rule.freq = freq?;
    let yearly_ordinal_without_month =
        rule.freq == Freq::Yearly && rule.by_month.is_empty() && rule.by_day.iter().any(|(n, _)| n.is_some());
    if yearly_ordinal_without_month || (rule.freq == Freq::Yearly && !rule.by_set_pos.is_empty()) {
        return None;
    }
    Some(rule)
}

fn month_day_matches(date: NaiveDate, days: &[i32]) -> bool {
    let dim = days_in_month(date.year(), date.month()) as i32;
    days.iter().any(|&d| if d > 0 { date.day() as i32 == d } else { date.day() as i32 == dim + d + 1 })
}

fn weekday_in_month_matches(date: NaiveDate, by_day: &[(Option<i32>, Weekday)]) -> bool {
    let dim = days_in_month(date.year(), date.month());
    by_day.iter().any(|&(n, weekday)| {
        date.weekday() == weekday
            && match n {
                None => true,
                Some(n) if n > 0 => ((date.day() - 1) / 7 + 1) as i32 == n,
                Some(n) => ((dim - date.day()) / 7 + 1) as i32 == -n,
            }
    })
}

impl RRule {
    /// The month-level filter shared by MONTHLY and YEARLY.
    fn day_in_month(&self, date: NaiveDate, start: NaiveDate) -> bool {
        if !self.by_month_day.is_empty() {
            month_day_matches(date, &self.by_month_day)
                && (self.by_day.is_empty() || self.by_day.iter().any(|(_, w)| *w == date.weekday()))
        } else if !self.by_day.is_empty() {
            weekday_in_month_matches(date, &self.by_day)
        } else {
            date.day() == start.day()
        }
    }

    fn matches(&self, date: NaiveDate, start: NaiveDate) -> bool {
        if date < start {
            return false;
        }
        if date == start {
            return true;
        }
        if !self.by_month.is_empty() && !self.by_month.contains(&date.month()) {
            return false;
        }
        match self.freq {
            Freq::Daily => {
                (date - start).num_days() % self.interval == 0
                    && (self.by_day.is_empty() || self.by_day.iter().any(|(_, w)| *w == date.weekday()))
                    && (self.by_month_day.is_empty() || month_day_matches(date, &self.by_month_day))
            }
            Freq::Weekly => {
                let week_start = |d: NaiveDate| d - Duration::days(d.weekday().days_since(self.wkst) as i64);
                let weeks = (week_start(date) - week_start(start)).num_days() / 7;
                let days: Vec<Weekday> = if self.by_day.is_empty() {
                    vec![start.weekday()]
                } else {
                    self.by_day.iter().map(|(_, w)| *w).collect()
                };
                weeks % self.interval == 0 && days.contains(&date.weekday())
            }
            Freq::Monthly => {
                let months = (date.year() - start.year()) as i64 * 12 + date.month() as i64 - start.month() as i64;
                if months % self.interval != 0 {
                    return false;
                }
                if self.by_set_pos.is_empty() {
                    return self.day_in_month(date, start);
                }
                let dim = days_in_month(date.year(), date.month());
                let set: Vec<NaiveDate> = (1..=dim)
                    .filter_map(|d| NaiveDate::from_ymd_opt(date.year(), date.month(), d))
                    .filter(|d| self.day_in_month(*d, start))
                    .collect();
                self.by_set_pos.iter().any(|&pos| {
                    let idx = if pos > 0 { pos as i64 - 1 } else { set.len() as i64 + pos as i64 };
                    idx >= 0 && set.get(idx as usize) == Some(&date)
                })
            }
            Freq::Yearly => {
                if (date.year() - start.year()) as i64 % self.interval != 0 {
                    return false;
                }
                if self.by_month.is_empty() && date.month() != start.month() {
                    return false;
                }
                self.day_in_month(date, start)
            }
        }
    }
}

/// Local start times of a recurring event's occurrences up to `until_local`,
/// honouring COUNT and UNTIL. Without COUNT the walk starts near `from_local`.
fn expand(
    rule: &RRule,
    dtstart: NaiveDateTime,
    zone: &Zone,
    from_local: NaiveDate,
    until_local: NaiveDate,
) -> Vec<NaiveDateTime> {
    let start = dtstart.date();
    let mut day = if rule.count.is_some() { start } else { start.max(from_local) };
    let mut out = Vec::new();
    let mut seen = 0u32;
    let mut walked = 0i64;
    while day <= until_local && walked < MAX_RECURRENCE_DAYS {
        walked += 1;
        if rule.matches(day, start) {
            let local = day.and_time(dtstart.time());
            seen += 1;
            if rule.count.is_some_and(|c| seen > c) {
                break;
            }
            let past_until = match rule.until {
                Some(IcsTime::Date(d)) => day > d,
                Some(IcsTime::Utc(u)) => zone.to_utc(local).is_some_and(|t| t > u),
                Some(IcsTime::Local(l)) => local > l,
                None => false,
            };
            if past_until {
                break;
            }
            if day >= from_local {
                out.push(local);
            }
        }
        day = match day.succ_opt() {
            Some(next) => next,
            None => break,
        };
    }
    out
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// What a feed gives Meetings: its name and the timed events inside the
/// window that carry a Meet, Zoom or Teams link.
#[derive(Debug, Default)]
pub struct ParsedFeed {
    pub name: Option<String>,
    pub events: Vec<CalendarEvent>,
}

/// The meeting link of an event: structured fields first (Teams and Google
/// conference properties, URL), then LOCATION, then DESCRIPTION.
fn event_link(event: &Component) -> Option<meeting_links::ParsedLink> {
    for name in ["X-MICROSOFT-SKYPETEAMSMEETINGURL", "X-GOOGLE-CONFERENCE", "URL"] {
        if let Some(link) = event.text(name).and_then(|v| meeting_links::parse_link(v.trim())) {
            return Some(link);
        }
    }
    ["LOCATION", "DESCRIPTION", "X-ALT-DESC"]
        .into_iter()
        .filter_map(|name| event.text(name))
        .find_map(|text| meeting_links::find_in_text(&super::decode_entities(&text)))
}

fn mail_address(value: &str) -> Option<String> {
    let v = value.trim();
    let addr = if v.len() >= 7 && v[..7].eq_ignore_ascii_case("mailto:") { &v[7..] } else { v };
    let addr = addr.trim().to_ascii_lowercase();
    addr.contains('@').then_some(addr)
}

/// ATTENDEE lines minus rooms and resources, plus the ORGANIZER when the
/// list doesn't already name them. No ATTENDEE lines = no attendee list.
fn attendees(event: &Component) -> Vec<Attendee> {
    let mut list: Vec<Attendee> = Vec::new();
    for prop in event.props("ATTENDEE") {
        let cutype = prop.param("CUTYPE").unwrap_or("INDIVIDUAL").to_ascii_uppercase();
        if cutype == "ROOM" || cutype == "RESOURCE" {
            continue;
        }
        let email = mail_address(&prop.value);
        if email.is_some() && list.iter().any(|a| a.email == email) {
            continue;
        }
        list.push(Attendee {
            email,
            name: prop.param("CN").map(|c| c.trim_matches('"').to_string()),
            response: prop.param("PARTSTAT").map(str::to_ascii_lowercase),
            is_self: false,
        });
    }
    if !list.is_empty() {
        if let Some(org) = event.prop("ORGANIZER") {
            let email = mail_address(&org.value);
            if email.is_some() && !list.iter().any(|a| a.email == email) {
                list.push(Attendee {
                    email,
                    name: org.param("CN").map(|c| c.trim_matches('"').to_string()),
                    response: Some("accepted".to_string()),
                    is_self: false,
                });
            }
        }
    }
    list
}

fn is_cancelled(event: &Component) -> bool {
    event.prop("STATUS").is_some_and(|p| p.value.trim().eq_ignore_ascii_case("CANCELLED"))
}

/// DTSTART as local wall time + zone (None for all-day or unparseable).
fn timed_start(event: &Component, zones: &Zones) -> Option<(NaiveDateTime, Zone, DateTime<Utc>)> {
    let prop = event.prop("DTSTART")?;
    let (times, tzid) = prop_times(prop);
    match *times.first()? {
        IcsTime::Date(_) => None,
        IcsTime::Utc(t) => Some((t.naive_utc(), Zone::Fixed(0), t)),
        IcsTime::Local(local) => {
            let zone = zones.zone(tzid.as_deref());
            let utc = zone.to_utc(local)?;
            Some((local, zone, utc))
        }
    }
}

/// Event length from DTEND or DURATION (zero when neither is given).
fn event_length(event: &Component, zones: &Zones, start_utc: DateTime<Utc>) -> Duration {
    if let Some(prop) = event.prop("DTEND") {
        let (times, tzid) = prop_times(prop);
        if let Some(end) = times.first().and_then(|t| zones.resolve(*t, tzid.as_deref())) {
            if end >= start_utc {
                return end - start_utc;
            }
        }
    }
    event.prop("DURATION").and_then(|p| parse_duration(&p.value)).unwrap_or_else(Duration::zero)
}

/// `PT1H30M`, `P1D`, `P1W`, `-PT15M` (negative durations count as zero).
fn parse_duration(value: &str) -> Option<Duration> {
    let v = value.trim().to_ascii_uppercase();
    if v.starts_with('-') {
        return Some(Duration::zero());
    }
    let v = v.trim_start_matches('+').strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for ch in v.chars() {
        match ch {
            '0'..='9' => num.push(ch),
            'T' => in_time = true,
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (unit, in_time) {
                    ('W', _) => 7 * 86_400,
                    ('D', _) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(Duration::seconds(total))
}

fn instance_key(uid: &str, original: DateTime<Utc>) -> String {
    format!("{uid}/{}", original.format("%Y%m%dT%H%M%SZ"))
}

fn build_event(
    event: &Component,
    external_id: String,
    starts_at: DateTime<Utc>,
    length: Duration,
) -> Option<CalendarEvent> {
    let link = event_link(event)?;
    let attendees = attendees(event);
    let attendee_count = (!attendees.is_empty()).then(|| attendees.len() as i32);
    Some(CalendarEvent {
        external_id,
        title: event.text("SUMMARY").map(|s| s.trim().to_string()).unwrap_or_default(),
        starts_at,
        ends_at: (length > Duration::zero()).then(|| starts_at + length),
        link,
        attendee_count,
        // The feed doesn't say which attendee is the calendar's owner, so
        // "outside" can't be told apart here.
        external_attendee_count: None,
        attendees,
    })
}

/// Parse a feed and return the timed, linked events that start inside
/// `[from, to]`. Fails only when the body is not an iCalendar object.
pub fn parse_feed(body: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<ParsedFeed> {
    let calendar = parse_components(body)?;
    let zones = Zones::new(&calendar);
    let name = calendar.text("X-WR-CALNAME").map(|s| s.trim().to_string());
    let vevents: Vec<&Component> = calendar.children.iter().filter(|c| c.name == "VEVENT").collect();

    // RECURRENCE-ID overrides by UID, keyed by the original start (UTC).
    let mut overrides: HashMap<&str, HashMap<DateTime<Utc>, &Component>> = HashMap::new();
    for ev in &vevents {
        let (Some(uid), Some(rid)) = (ev.prop("UID"), ev.prop("RECURRENCE-ID")) else { continue };
        let (times, tzid) = prop_times(rid);
        if let Some(original) = times.first().and_then(|t| zones.resolve(*t, tzid.as_deref())) {
            overrides.entry(uid.value.trim()).or_default().insert(original, ev);
        }
    }

    let in_window = |t: DateTime<Utc>| t >= from && t <= to;
    let mut events = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut push = |event: Option<CalendarEvent>, events: &mut Vec<CalendarEvent>| {
        if let Some(event) = event {
            if seen_ids.insert(event.external_id.clone()) {
                events.push(event);
            }
        }
    };

    for ev in &vevents {
        if ev.prop("RECURRENCE-ID").is_some() {
            continue;
        }
        let Some(uid) = ev.prop("UID").map(|p| p.value.trim()).filter(|u| !u.is_empty()) else { continue };
        if is_cancelled(ev) {
            continue;
        }
        let Some((local, zone, start_utc)) = timed_start(ev, &zones) else { continue };
        let length = event_length(ev, &zones, start_utc);
        let rrule = ev.prop("RRULE").map(|p| p.value.as_str());
        let has_rdate = ev.prop("RDATE").is_some();

        if rrule.is_none() && !has_rdate {
            if in_window(start_utc) {
                push(build_event(ev, uid.to_string(), start_utc, length), &mut events);
            }
            continue;
        }

        // Exclusions: exact instants, or whole local dates for DATE values.
        let mut ex_instants: HashSet<DateTime<Utc>> = HashSet::new();
        let mut ex_dates: HashSet<NaiveDate> = HashSet::new();
        for prop in ev.props("EXDATE") {
            let (times, tzid) = prop_times(prop);
            for t in times {
                match t {
                    IcsTime::Date(d) => {
                        ex_dates.insert(d);
                    }
                    other => {
                        if let Some(at) = zones.resolve(other, tzid.as_deref()) {
                            ex_instants.insert(at);
                        }
                    }
                }
            }
        }

        let from_local = zone.to_local(from).date() - Duration::days(1);
        let to_local = zone.to_local(to).date() + Duration::days(1);
        let mut starts: Vec<DateTime<Utc>> = match rrule {
            Some(value) => match parse_rrule(value) {
                Some(rule) => expand(&rule, local, &zone, from_local, to_local)
                    .into_iter()
                    .filter(|l| !ex_dates.contains(&l.date()))
                    .filter_map(|l| zone.to_utc(l))
                    .collect(),
                None => {
                    tracing::debug!("meeting calendar: RRULE {value:?} not expanded; first occurrence only");
                    vec![start_utc]
                }
            },
            None => vec![start_utc],
        };
        for prop in ev.props("RDATE") {
            if prop.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("PERIOD")) {
                continue;
            }
            let (times, tzid) = prop_times(prop);
            starts.extend(times.into_iter().filter_map(|t| zones.resolve(t, tzid.as_deref())));
        }
        starts.sort();
        starts.dedup();

        let uid_overrides = overrides.get(uid);
        for original in starts {
            if ex_instants.contains(&original) {
                continue;
            }
            if uid_overrides.is_some_and(|o| o.contains_key(&original)) {
                continue; // emitted from the override below
            }
            if in_window(original) {
                push(build_event(ev, instance_key(uid, original), original, length), &mut events);
            }
        }
    }

    // Overrides (moved or edited instances); cancelled ones drop out.
    for (uid, by_original) in &overrides {
        for (original, ev) in by_original {
            if is_cancelled(ev) {
                continue;
            }
            let Some((_, _, start_utc)) = timed_start(ev, &zones) else { continue };
            if in_window(start_utc) {
                let length = event_length(ev, &zones, start_utc);
                push(build_event(ev, instance_key(uid, *original), start_utc, length), &mut events);
            }
        }
    }

    events.sort_by_key(|e| e.starts_at);
    Some(ParsedFeed { name, events })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTLOOK: &str = include_str!("fixtures/outlook.ics");
    const ICLOUD: &str = include_str!("fixtures/icloud.ics");

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        (utc("2026-11-16T00:00:00Z"), utc("2026-11-23T00:00:00Z"))
    }

    fn by_title<'a>(feed: &'a ParsedFeed, title: &str) -> Vec<&'a CalendarEvent> {
        feed.events.iter().filter(|e| e.title == title).collect()
    }

    #[test]
    fn calendar_outlook_feed_reads_teams_links_windows_zones_and_exdates() {
        let (from, to) = window();
        let feed = parse_feed(OUTLOOK, from, to).expect("parses");
        assert_eq!(feed.name.as_deref(), Some("Calendar"));

        // Weekly Mon/Wed/Fri standup, 09:30 Pacific (PST = UTC-8 in late
        // November), EXDATE on Wednesday the 18th, and an override that moves
        // Friday the 20th to 11:00.
        let standup = by_title(&feed, "Team standup");
        let starts: Vec<String> = standup.iter().map(|e| e.starts_at.to_rfc3339()).collect();
        assert_eq!(standup.len(), 2, "Mon 16th + moved Fri 20th; Wed 18th is excluded: {starts:?}");
        assert_eq!(standup[0].starts_at, utc("2026-11-16T17:30:00Z"));
        assert_eq!(standup[1].starts_at, utc("2026-11-20T19:00:00Z"), "override moved it to 11:00 PST");
        assert!(standup[1].external_id.ends_with("/20261120T173000Z"), "keyed by the original start: {}", standup[1].external_id);
        assert_eq!(standup[0].link.platform, meeting_links::Platform::Teams);
        assert_eq!(standup[0].ends_at, Some(utc("2026-11-16T17:45:00Z")));
        assert_eq!(standup[0].attendee_count, Some(4), "organizer + 3 people, the room excluded");
        assert!(!meeting_links::is_one_on_one(standup[0].attendee_count));

        // A one-off 1:1 in Eastern time with the Teams link only in the X- prop.
        let one = by_title(&feed, "Jordan / Sam 1:1");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].starts_at, utc("2026-11-17T19:00:00Z"));
        assert_eq!(one[0].attendee_count, Some(2));
        assert!(meeting_links::is_one_on_one(one[0].attendee_count));
        assert_eq!(one[0].external_id, "040000008200E00074C5B7101A82E0080000000010B1A1C1D1E1F1");

        // A custom "(UTC+01:00) ..." TZID with its own VTIMEZONE: 10:00 CET.
        let review = by_title(&feed, "Quarterly review");
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].starts_at, utc("2026-11-19T09:00:00Z"));
        assert_eq!(review[0].link.url_key, "zoom:85512345678");
        assert_eq!(review[0].attendee_count, None, "no ATTENDEE lines = no list = not 1:1");

        // All-day, cancelled and link-less events are not meetings.
        assert!(by_title(&feed, "Company holiday").is_empty());
        assert!(by_title(&feed, "Cancelled sync").is_empty());
        assert!(by_title(&feed, "Lunch").is_empty());
    }

    #[test]
    fn calendar_icloud_feed_reads_iana_zones_meet_and_zoom_links_and_counts() {
        let (from, to) = window();
        let feed = parse_feed(ICLOUD, from, to).expect("parses");
        assert_eq!(feed.name.as_deref(), Some("Work, shared"));

        // Daily COUNT=5 from Thu 2026-11-12 08:00 Berlin: the 12th..16th, so
        // only the 16th (Mon) is inside the window. 08:00 CET = 07:00Z.
        let daily = by_title(&feed, "Design crit");
        assert_eq!(daily.len(), 1, "{:?}", daily.iter().map(|e| e.starts_at).collect::<Vec<_>>());
        assert_eq!(daily[0].starts_at, utc("2026-11-16T07:00:00Z"));
        assert_eq!(daily[0].link.url_key, "meet:abc-defg-hij", "Meet link from DESCRIPTION");

        // Monthly on the 3rd Thursday (Nov 19) in New York via a mozilla-style
        // TZID path; Zoom link in LOCATION with an escaped comma.
        let monthly = by_title(&feed, "Board, monthly");
        assert_eq!(monthly.len(), 1);
        assert_eq!(monthly[0].starts_at, utc("2026-11-19T22:00:00Z"));
        assert_eq!(monthly[0].link.url_key, "zoom:85598765432");
        assert_eq!(monthly[0].link.join_url, "https://us02web.zoom.us/j/85598765432?pwd=QWERTY");

        // A UTC one-off with the link in URL and a folded DESCRIPTION.
        let utc_one = by_title(&feed, "Vendor call");
        assert_eq!(utc_one.len(), 1);
        assert_eq!(utc_one[0].starts_at, utc("2026-11-18T15:00:00Z"));
        assert_eq!(utc_one[0].link.url_key, "meet:xyz-abcd-efg");
        assert_eq!(utc_one[0].ends_at, Some(utc("2026-11-18T15:30:00Z")), "DURATION PT30M");

        // UNTIL before the window ends the series; a cancelled override
        // removes Tuesday's instance of the weekly sync.
        assert!(by_title(&feed, "Old series").is_empty());
        let weekly = by_title(&feed, "Weekly sync");
        let days: Vec<u32> = weekly.iter().map(|e| e.starts_at.day()).collect();
        assert_eq!(days, vec![19], "Tue 17th cancelled by its override, Thu 19th kept");
        assert_eq!(weekly[0].starts_at, utc("2026-11-19T17:00:00Z"), "09:00 Los Angeles");
    }

    #[test]
    fn calendar_feed_check_needs_a_vcalendar() {
        assert!(is_vcalendar(OUTLOOK));
        assert!(is_vcalendar(&format!("\u{feff}{ICLOUD}")));
        assert!(!is_vcalendar("<html><body>Sign in</body></html>"));
        assert!(!is_vcalendar("BEGIN:VCARD\nEND:VCARD\n"));
        assert!(!is_vcalendar(""));
    }

    #[test]
    fn calendar_dst_switch_moves_the_utc_time_of_a_weekly_event() {
        // 09:00 New York every Monday across the Nov 1 2026 switch:
        // EDT (UTC-4) before, EST (UTC-5) after.
        let body = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:dst\r\nSUMMARY:Mon\r\n\
DTSTART;TZID=America/New_York:20261026T090000\r\nDTEND;TZID=America/New_York:20261026T093000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\nLOCATION:https://meet.google.com/aaa-bbbb-ccc\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let feed = parse_feed(body, utc("2026-10-25T00:00:00Z"), utc("2026-11-03T00:00:00Z")).unwrap();
        let starts: Vec<DateTime<Utc>> = feed.events.iter().map(|e| e.starts_at).collect();
        assert_eq!(starts, vec![utc("2026-10-26T13:00:00Z"), utc("2026-11-02T14:00:00Z")]);
    }

    #[test]
    fn calendar_feed_vtimezone_without_a_known_name_follows_its_rules() {
        let body = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:My Office\r\nBEGIN:STANDARD\r\n\
DTSTART:16010101T020000\r\nTZOFFSETFROM:-0700\r\nTZOFFSETTO:-0800\r\nRRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11\r\n\
END:STANDARD\r\nBEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:-0800\r\nTZOFFSETTO:-0700\r\n\
RRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Summer\r\nDTSTART;TZID=My Office:20260715T100000\r\nURL:https://zoom.us/j/855123456\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Winter\r\nDTSTART;TZID=\"My Office\":20261215T100000\r\nURL:https://zoom.us/j/855123457\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let feed = parse_feed(body, utc("2026-01-01T00:00:00Z"), utc("2027-01-01T00:00:00Z")).unwrap();
        let summer = by_title(&feed, "Summer");
        let winter = by_title(&feed, "Winter");
        assert_eq!(summer[0].starts_at, utc("2026-07-15T17:00:00Z"), "PDT -7");
        assert_eq!(winter[0].starts_at, utc("2026-12-15T18:00:00Z"), "PST -8");
    }
}
