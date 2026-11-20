//! Port of the French date filters: `org.languagetool.rules.fr.DateCheckFilter`
//! (`org.languagetool.rules.AbstractDateCheckWithSuggestionsFilter`: "lundi 3
//! mars 2020" when that day was a Tuesday), `NewYearDateFilter`
//! (`AbstractNewYearDateFilter`: last year's date written in January) and
//! their `DateFilterHelper`.
//!
//! Java's non-lenient `GregorianCalendar` is replaced by the date arithmetic
//! below, which follows it: Julian calendar before 15 October 1582, and
//! impossible dates are rejected.

use std::collections::HashMap;
use std::sync::Arc;

use super::{parse_int, rebuilt_match, FilterEnv, Today};
use crate::engine::rule::{skip_corrected_reference, RuleContext, RuleFilter, RuleMatch};
use crate::engine::strtools;
use crate::engine::token::{Readings, Sentence};

/// `Calendar.getDisplayName(DAY_OF_WEEK, LONG, Locale.FRENCH)`, from
/// `Calendar.SUNDAY` (1) to `Calendar.SATURDAY` (7).
const DAY_NAMES: [&str; 7] = ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"];

/// Julian day number of a Gregorian (or, before the 1582 reform, Julian)
/// calendar date. `month` is 1 to 12.
fn julian_day_number(year: i64, month: i64, day: i64) -> i64 {
    let a = (14 - month) / 12;
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    let base = day + (153 * m + 2) / 5 + 365 * y + y / 4;
    if is_gregorian(year, month, day) {
        base - y / 100 + y / 400 - 32045
    } else {
        base - 32083
    }
}

/// Java's `GregorianCalendar` switches from the Julian calendar on
/// 15 October 1582.
fn is_gregorian(year: i64, month: i64, day: i64) -> bool {
    (year, month, day) >= (1582, 10, 15)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let leap = if year > 1582 { year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) } else { year % 4 == 0 };
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A date as a non-lenient `Calendar` holds it: `month0` is 0 to 11.
#[derive(Clone, Copy)]
struct Date {
    year: i64,
    month0: i64,
    day: i64,
}

impl Date {
    /// Does the date exist? (`Calendar.get` throws otherwise.)
    fn is_valid(self) -> bool {
        let month = self.month0 + 1;
        if self.year < 1 || !(1..=12).contains(&month) || self.day < 1 || self.day > days_in_month(self.year, month) {
            return false;
        }
        // The ten days dropped by the Gregorian reform.
        !(self.year == 1582 && month == 10 && (5..15).contains(&self.day))
    }

    /// `Calendar.get(DAY_OF_WEEK)`: 1 (Sunday) to 7 (Saturday); `None` for an
    /// impossible date.
    fn day_of_week(self) -> Option<i64> {
        if !self.is_valid() {
            return None;
        }
        Some((julian_day_number(self.year, self.month0 + 1, self.day) + 1).rem_euclid(7) + 1)
    }
}

/// The civil (Gregorian) date of a day count since 1970-01-01.
pub(crate) fn civil_from_days(days: i64) -> Today {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Today { year, month: month as u32, day: day as u32 }
}

/// `DateFilterHelper.getDayOfWeek(String)`: `Calendar.SUNDAY`...
fn day_of_week_from_name(day: &str) -> Option<i64> {
    let day = day.to_lowercase();
    ["dim", "lun", "mar", "mer", "jeu", "ven", "sam"].iter().position(|p| day.starts_with(p)).map(|i| i as i64 + 1)
}

/// `DateFilterHelper.getMonth`: 1 to 12.
fn month_from_name(month: &str) -> Option<i64> {
    let mon = month.to_lowercase();
    const PREFIXES: [(&str, i64); 15] = [
        ("jan", 1),
        ("fév", 2),
        ("fev", 2),
        ("mar", 3),
        ("avr", 4),
        ("mai", 5),
        ("juin", 6),
        ("juil", 7),
        ("aou", 8),
        ("aoû", 8),
        ("sep", 9),
        ("oct", 10),
        ("nov", 11),
        ("déc", 12),
        ("dec", 12),
    ];
    PREFIXES.iter().find(|(p, _)| mon.starts_with(p)).map(|(_, n)| *n)
}

/// `StringTools.trimSpecialCharacters`: keeps letters, digits, whitespace and
/// ASCII punctuation (soft hyphens and the like are removed).
fn trim_special_characters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace() || c.is_ascii_punctuation()).collect()
}

/// `StringUtils.isNumeric`.
fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// `getDayOfMonthFromStr`: the leading digits (`(\d+).*`), else 0.
fn day_of_month(s: &str) -> Option<i64> {
    let digits: &str = &s[..s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len())];
    if digits.is_empty() {
        return Some(0);
    }
    parse_int(digits)
}

/// `getMonthFromStr` / `getMonthFromArguments`: 0 to 11.
fn month0(s: &str, trim: bool) -> Option<i64> {
    if is_numeric(s) {
        return parse_int(s).map(|m| m - 1);
    }
    let name = if trim { trim_special_characters(s) } else { s.to_string() };
    month_from_name(&name).map(|m| m - 1)
}

pub(crate) struct DateCheckFilter {
    env: Arc<FilterEnv>,
}

impl DateCheckFilter {
    pub(crate) fn new(env: Arc<FilterEnv>) -> DateCheckFilter {
        DateCheckFilter { env }
    }
}

/// `findNewDayOfMonth`: the nearest day of the month that falls on the
/// written week day.
fn find_new_day_of_month(day: i64, month0: i64, year: i64, wanted_day_of_week: i64) -> Option<i64> {
    for difference in 1..7 {
        for candidate in [day - difference, day + difference] {
            if candidate > 0 && candidate < 32 && (Date { year, month0, day: candidate }).day_of_week() == Some(wanted_day_of_week) {
                return Some(candidate);
            }
        }
    }
    None
}

impl RuleFilter for DateCheckFilter {
    fn accept(
        &self,
        m: RuleMatch,
        args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        pattern_tokens: &[&Readings],
        token_positions: &[usize],
        sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        // The arguments are pattern element numbers. A reference that
        // points nowhere rejects the match.
        let reference = |key: &str, default: Option<i64>| -> Option<i64> {
            let n = match args.get(key) {
                Some(v) => parse_int(v)?,
                None => default?,
            };
            Some(skip_corrected_reference(token_positions, n))
        };
        let token_at = |pos: i64| -> Option<&Readings> { usize::try_from(pos).ok().and_then(|p| pattern_tokens.get(p)).copied() };

        let day_of_week_pos = reference("weekDay", None)?;
        let day_of_week_str = token_at(day_of_week_pos)?.token.replace('\u{ad}', "");
        let full_date_pos = reference("date", Some(-1))?;
        let is_full_date_token = full_date_pos > -1;
        let (day_pos, year_pos, day_str, month_str, year_str): (i64, i64, String, String, Option<String>) = if is_full_date_token {
            // One token "yyyy-mm-dd".
            let token = token_at(full_date_pos)?;
            let parts: Vec<&str> = token.token.split('-').collect();
            (full_date_pos, full_date_pos, parts.get(2)?.to_string(), parts.get(1)?.to_string(), Some(parts.first()?.to_string()))
        } else {
            let day_pos = reference("day", None)?;
            let month_pos = reference("month", None)?;
            let year_pos = reference("year", Some(-1))?;
            let year_str = if year_pos > -1 { Some(token_at(year_pos)?.token.to_string()) } else { None };
            (day_pos, year_pos, token_at(day_pos)?.token.to_string(), token_at(month_pos)?.token.to_string(), year_str)
        };

        let day_of_week_from_string = day_of_week_from_name(&day_of_week_str)?;
        let day = day_of_month(&day_str)?;
        let month0 = month0(&month_str, true)?;
        let current_year = self.env.today().year;
        let year = match &year_str {
            Some(y) => parse_int(y)?,
            None => current_year,
        };
        let date = Date { year, month0, day };
        let day_of_week_from_date = date.day_of_week()?;
        if day_of_week_from_string == day_of_week_from_date {
            return None;
        }
        // The week day is right for this year: the year is the mistake.
        if (Date { year: current_year, month0, day }).day_of_week() == Some(day_of_week_from_string) {
            let year_token = token_at(year_pos)?;
            let message = format!("Cette date est incorrecte. Faites-vous référence à l'année \"{current_year}\" ?");
            let mut rule_match = rebuilt_match(&m, sentence, year_token.start_pos, year_token.end_pos(), message);
            rule_match.suggestions = vec![if is_full_date_token { format!("{current_year}-{month_str}-{day_str}") } else { current_year.to_string() }];
            return Some(rule_match);
        }
        let real_day = DAY_NAMES[day_of_week_from_date as usize - 1];
        let message = m
            .message
            .replace("{realDay}", real_day)
            .replace("{day}", DAY_NAMES[day_of_week_from_string as usize - 1])
            .replace("{currentYear}", &current_year.to_string());
        let (start_index, end_index) = if day_of_week_pos < day_pos { (day_of_week_pos, day_pos) } else { (day_pos, day_of_week_pos) };
        let (start_token, end_token) = (token_at(start_index)?, token_at(end_index)?);
        if end_token.end_pos() <= start_token.start_pos {
            return None;
        }
        let mut rule_match = rebuilt_match(&m, sentence, start_token.start_pos, end_token.end_pos(), message);
        // The tokens from the week day to the day of the month, one of them
        // replaced.
        let rewrite = |replaced: i64, replacement: &str| -> String {
            let mut suggestion = String::new();
            for j in start_index..=end_index {
                let Some(token) = token_at(j) else { continue };
                if j > start_index && token.whitespace_before {
                    suggestion.push(' ');
                }
                suggestion.push_str(if j == replaced { replacement } else { &token.token });
            }
            suggestion
        };
        let suggestion = rewrite(day_of_week_pos, &strtools::preserve_case(real_day, &day_of_week_str));
        if !suggestion.is_empty() {
            rule_match.suggestions = vec![suggestion];
        }
        if let Some(corrected) = find_new_day_of_month(day, month0, year, day_of_week_from_string) {
            let new_day = match (&year_str, is_full_date_token) {
                (Some(y), true) => format!("{y}-{month_str}-{corrected}"),
                _ => corrected.to_string(),
            };
            let suggestion = rewrite(day_pos, &new_day);
            if !suggestion.is_empty() {
                rule_match.suggestions.push(suggestion);
            }
        }
        Some(rule_match)
    }
}

pub(crate) struct NewYearDateFilter {
    env: Arc<FilterEnv>,
}

impl NewYearDateFilter {
    pub(crate) fn new(env: Arc<FilterEnv>) -> NewYearDateFilter {
        NewYearDateFilter { env }
    }
}

impl RuleFilter for NewYearDateFilter {
    fn accept(
        &self,
        m: RuleMatch,
        args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        _pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        // Here the arguments are the texts of the matched tokens.
        let year = parse_int(args.get("year")?)?;
        let month0 = month0(&args.get("month")?.replace('\u{ad}', ""), false)?;
        let day = day_of_month(&args.get("day")?.replace('\u{ad}', ""))?;
        // An impossible date is another rule's business.
        if !(Date { year, month0, day }).is_valid() {
            return None;
        }
        let today = self.env.today();
        if today.month == 1 && month0 != 11 && year + 1 == today.year {
            let message = m.message.replace("{year}", &year.to_string()).replace("{realYear}", &today.year.to_string());
            return Some(rebuilt_match(&m, sentence, m.from_pos, m.to_pos, message));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;
    use crate::engine::filters::FrenchFilters;

    fn dow(year: i64, month: i64, day: i64) -> Option<i64> {
        (Date { year, month0: month - 1, day }).day_of_week()
    }

    #[test]
    fn day_of_week_follows_the_java_calendar() {
        assert_eq!(dow(1970, 1, 1), Some(5)); // Thursday
        assert_eq!(dow(2000, 2, 29), Some(3)); // Tuesday
        assert_eq!(dow(2024, 7, 14), Some(1)); // Sunday
        assert_eq!(dow(2026, 11, 20), Some(6)); // Friday
        assert_eq!(dow(1789, 7, 14), Some(3)); // Tuesday
        // Either side of the Gregorian reform: Thursday, then Friday.
        assert_eq!(dow(1582, 10, 4), Some(5));
        assert_eq!(dow(1582, 10, 15), Some(6));
        assert_eq!(dow(1582, 10, 10), None);
        // Julian calendar before that: 1500 is a leap year.
        assert!(dow(1500, 2, 29).is_some());
        assert_eq!(dow(1900, 2, 29), None);
        assert_eq!(dow(2023, 2, 29), None);
        assert_eq!(dow(2023, 13, 1), None);
        assert_eq!(dow(2023, 4, 31), None);
        assert_eq!(dow(0, 1, 1), None);
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), Today { year: 1970, month: 1, day: 1 });
        assert_eq!(civil_from_days(19_782), Today { year: 2024, month: 2, day: 29 });
        assert_eq!(civil_from_days(-1), Today { year: 1969, month: 12, day: 31 });
    }

    #[test]
    fn names() {
        assert_eq!(day_of_week_from_name("Mardi"), Some(3));
        assert_eq!(day_of_week_from_name("mercredis"), Some(4));
        assert_eq!(day_of_week_from_name("demain"), None);
        assert_eq!(month_from_name("Février"), Some(2));
        assert_eq!(month_from_name("aout"), Some(8));
        assert_eq!(month_from_name("juillet"), Some(7));
        assert_eq!(month_from_name("juin"), Some(6));
        assert_eq!(month_from_name("lundi"), None);
        assert_eq!(day_of_month("1er"), Some(1));
        assert_eq!(day_of_month("premier"), Some(0));
        assert_eq!(trim_special_characters("dé\u{ad}cembre"), "décembre");
    }

    fn date_filter(year: i64) -> DateCheckFilter {
        let filters = FrenchFilters::new();
        filters.set_today(Some(Today { year, month: 6, day: 1 }));
        DateCheckFilter::new(filters.env.clone())
    }

    const MESSAGE: &str = "Le {day} ne correspond pas au {realDay}.";

    #[test]
    fn wrong_week_day() {
        // 3 mars 2020 was a Tuesday.
        let s = sentence(&[("Lundi", &[]), ("3", &[]), ("mars", &[]), ("2020", &[])]);
        let tokens = s.non_ws_tokens();
        let m = match_over(&s, 1, 4, MESSAGE);
        let a = args(&[("weekDay", "1"), ("day", "2"), ("month", "3"), ("year", "4")]);
        let out = date_filter(2021).accept(m.clone(), &a, 1, &tokens[1..], &[1, 1, 1, 1], &s, &bare_context()).unwrap();
        assert_eq!(out.message, "Le lundi ne correspond pas au mardi.");
        assert_eq!(out.suggestions, ["Mardi 3", "Lundi 2"]);
        assert_eq!(&s.text[out.from_pos..out.to_pos], "Lundi 3");

        // The right day: no match.
        let s2 = sentence(&[("Mardi", &[]), ("3", &[]), ("mars", &[]), ("2020", &[])]);
        let m2 = match_over(&s2, 1, 4, MESSAGE);
        assert!(date_filter(2021).accept(m2, &a, 1, &s2.non_ws_tokens()[1..], &[1, 1, 1, 1], &s2, &bare_context()).is_none());

        // 3 mars 2025 was a Monday: writing "2020" in 2025 is a wrong year.
        let out = date_filter(2025).accept(m, &a, 1, &tokens[1..], &[1, 1, 1, 1], &s, &bare_context()).unwrap();
        assert_eq!(out.suggestions, ["2025"]);
        assert_eq!(&s.text[out.from_pos..out.to_pos], "2020");
        assert!(out.message.contains("\"2025\""));
    }

    #[test]
    fn impossible_dates_and_unknown_names_are_left_alone() {
        let a = args(&[("weekDay", "1"), ("day", "2"), ("month", "3"), ("year", "4")]);
        for words in [["Lundi", "31", "avril", "2020"], ["Lundi", "3", "brumaire", "2020"], ["Jour", "3", "mars", "2020"]] {
            let s = sentence(&[(words[0], &[]), (words[1], &[]), (words[2], &[]), (words[3], &[])]);
            let m = match_over(&s, 1, 4, MESSAGE);
            assert!(date_filter(2021).accept(m, &a, 1, &s.non_ws_tokens()[1..], &[1, 1, 1, 1], &s, &bare_context()).is_none(), "{words:?}");
        }
    }

    #[test]
    fn without_a_year_the_current_one_is_meant() {
        let s = sentence(&[("lundi", &[]), ("3", &[]), ("mars", &[])]);
        let m = match_over(&s, 1, 3, MESSAGE);
        let a = args(&[("weekDay", "1"), ("day", "2"), ("month", "3")]);
        // Monday in 2025, Tuesday in 2026.
        assert!(date_filter(2025).accept(m.clone(), &a, 1, &s.non_ws_tokens()[1..], &[1, 1, 1], &s, &bare_context()).is_none());
        let out = date_filter(2026).accept(m, &a, 1, &s.non_ws_tokens()[1..], &[1, 1, 1], &s, &bare_context()).unwrap();
        assert_eq!(out.suggestions, ["mardi 3", "lundi 2"]);
    }

    #[test]
    fn last_years_date_in_january() {
        let filters = FrenchFilters::new();
        let filter = NewYearDateFilter::new(filters.env.clone());
        let s = sentence(&[("3", &[]), ("janvier", &[]), ("2013", &[])]);
        let m = match_over(&s, 1, 3, "{year} au lieu de {realYear} ?");
        let january = args(&[("year", "2013"), ("month", "janvier"), ("day", "3")]);
        let december = args(&[("year", "2013"), ("month", "décembre"), ("day", "3")]);
        let accept = |a: &HashMap<String, String>| filter.accept(m.clone(), a, 1, &s.non_ws_tokens()[1..], &[1, 1, 1], &s, &bare_context());

        filters.set_today(Some(Today { year: 2014, month: 1, day: 10 }));
        assert_eq!(accept(&january).unwrap().message, "2013 au lieu de 2014 ?");
        assert!(accept(&december).is_none(), "a date in December of last year is plausible");
        filters.set_today(Some(Today { year: 2014, month: 2, day: 10 }));
        assert!(accept(&january).is_none(), "only in January");
        filters.set_today(Some(Today { year: 2016, month: 1, day: 10 }));
        assert!(accept(&january).is_none(), "only for last year");
    }
}
