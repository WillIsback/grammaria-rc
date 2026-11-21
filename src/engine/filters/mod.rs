//! Rule filters: the Java classes a rule file names in `<filter class=...>`
//! to accept, reject or rewrite a match (`org.languagetool.rules.patterns.RuleFilter`
//! and its subclasses in `org.languagetool.rules` and
//! `org.languagetool.rules.fr`, LanguageTool, LGPL-2.1-or-later).
//!
//! [`french_filter`] is the [`FilterFactory`](super::xmlrules::FilterFactory)
//! of the French rule files. Some filters need more than the match they are
//! given; they get it from the [`FrenchFilters`] they were created by:
//!
//! * `WordWithDeterminerFilter` and `SuppressIfAnyRuleMatchesFilter` analyse a
//!   corrected text and run other rules on it: [`FrenchFilters::set_analyzer`]
//!   and [`FrenchFilters::set_rules`];
//! * the date filters read the current date: [`FrenchFilters::set_today`];
//! * `IsEnglishWordFilter` tags English words:
//!   [`FrenchFilters::set_english_tagger`].
//!
//! `InterrogativeVerbFilter` and `FrenchNumberInWordFilter` use the speller
//! and the tagger of the language ([`Lang`](super::pattern::Lang)).
//!
//! Without a port: the speller-based `FindSuggestionsFilter`,
//! `FrenchSuppressMisspelledSuggestionsFilter` and `MultitokenSpellerFilter`,
//! and `DMYDateCheckFilter`, which no active rule uses.

pub mod analysis;
mod contractions;
mod dates;
mod determiner;
mod english;
mod interrogative;
mod number_in_word;
mod postponed_adjective;
mod suggestions;
mod suppress;
mod synthesizer;

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::pattern::WordTagger;
use super::rule::{PatternRule, RuleContext, RuleFilter, RuleMatch};
use super::token::{Reading, Readings, Sentence};
use super::xmlrules::GrammarRule;
use crate::re::{Flags, Regex};

pub use analysis::{SentenceAnalyzer, SimpleAnalyzer, SimpleDictTagger};

/// What `Reading::pos` is compared as when a reading has no tag.
pub(crate) const UNKNOWN: &str = "UNKNOWN";

/// A static, lazily compiled regular expression.
macro_rules! static_regex {
    ($name:ident, $src:expr) => {
        static_regex!($name, $src, crate::re::Flags::default());
    };
    ($name:ident, $src:expr, $flags:expr) => {
        fn $name() -> &'static crate::re::Regex {
            static RE: std::sync::OnceLock<crate::re::Regex> = std::sync::OnceLock::new();
            RE.get_or_init(|| crate::re::Regex::new($src, $flags).expect("valid regex"))
        }
    };
}
pub(crate) use static_regex;

/// A static [`TagPattern`].
macro_rules! tag_pattern {
    ($name:ident, $src:expr) => {
        fn $name() -> &'static crate::engine::filters::TagPattern {
            static P: std::sync::OnceLock<crate::engine::filters::TagPattern> =
                std::sync::OnceLock::new();
            P.get_or_init(|| crate::engine::filters::TagPattern::new($src))
        }
    };
}
pub(crate) use tag_pattern;

/// Number of tags whose verdict a [`TagPattern`] remembers.
const TAG_CACHE: usize = 1024;

/// A regular expression over part-of-speech tags (`Pattern.matcher(posTag)
/// .matches()`), with its verdict cached per tag.
pub(crate) struct TagPattern {
    regex: Regex,
    /// Verdict for the tag `UNKNOWN`, which stands for "no tag".
    unknown: bool,
    /// 0 unknown, 1 no, 2 yes, per tag id.
    cache: Box<[AtomicU8]>,
}

impl TagPattern {
    pub(crate) fn new(src: &str) -> TagPattern {
        let regex = Regex::new(src, Flags::default()).expect("valid tag regex");
        let unknown = regex.is_full_match(UNKNOWN);
        TagPattern {
            regex,
            unknown,
            cache: (0..TAG_CACHE).map(|_| AtomicU8::new(0)).collect(),
        }
    }

    /// Does the reading's tag match? A reading without a tag does not.
    #[inline]
    pub(crate) fn matches_tag(&self, r: &Reading) -> bool {
        let (Some(tag), Some(text)) = (r.pos_tag, r.pos()) else {
            return false;
        };
        match self.cache.get(tag.id() as usize) {
            Some(slot) => match slot.load(Ordering::Relaxed) {
                1 => false,
                2 => true,
                _ => {
                    let hit = self.regex.is_full_match(text);
                    slot.store(if hit { 2 } else { 1 }, Ordering::Relaxed);
                    hit
                }
            },
            None => self.regex.is_full_match(text),
        }
    }

    /// Same, a reading without a tag being tagged `UNKNOWN` (the private
    /// `matchPostagRegexp` helpers of the filters).
    #[inline]
    pub(crate) fn matches_reading(&self, r: &Reading) -> bool {
        if r.pos_tag.is_none() {
            return self.unknown;
        }
        self.matches_tag(r)
    }

    /// `matchPostagRegexp(token, pattern)`: some reading matches, `UNKNOWN`
    /// standing for no tag.
    #[inline]
    pub(crate) fn matches(&self, t: &Readings) -> bool {
        t.readings.iter().any(|r| self.matches_reading(r))
    }

    /// `getAnalyzedToken(token, pattern)`: the first matching reading.
    pub(crate) fn find<'a>(&self, t: &'a Readings) -> Option<&'a Reading> {
        t.readings.iter().find(|r| self.matches_reading(r))
    }

    /// `AnalyzedTokenReadings.matchesPosTagRegex`: untagged readings never
    /// match.
    pub(crate) fn matches_tagged(&self, t: &Readings) -> bool {
        t.readings.iter().any(|r| self.matches_tag(r))
    }
}

/// `new RuleMatch(match.getRule(), match.getSentence(), from, to, message,
/// match.getShortMessage())`: a match rebuilt from its message. Its
/// suggestions are those written in the message, in the case they were
/// written in.
pub(crate) fn rebuilt_match(
    m: &RuleMatch,
    sentence: &Sentence,
    from: usize,
    to: usize,
    message: String,
) -> RuleMatch {
    RuleMatch::new(
        m.rule,
        &sentence.text,
        from,
        to,
        from,
        to,
        message,
        m.short_message.clone(),
        false,
        false,
        "",
    )
}

/// `Integer.parseInt`.
pub(crate) fn parse_int(s: &str) -> Option<i64> {
    let digits = s
        .strip_prefix('-')
        .or_else(|| s.strip_prefix('+'))
        .unwrap_or(s);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.strip_prefix('+').unwrap_or(s).parse().ok()
}

/// A 1-based position among the pattern tokens, as the filters read it from
/// their arguments: a number, or `marker`, `marker1`, `marker-1`... relative
/// to the first token of the match. `skip_sentence_start` is the variant of
/// `RuleFilter.getPosition`. Returns the 0-based index.
pub(crate) fn token_position(
    from: &str,
    pattern_tokens: &[&Readings],
    m: &RuleMatch,
    skip_sentence_start: bool,
) -> Option<usize> {
    let position: i64 = if let Some(offset) = from.strip_prefix("marker") {
        let mut i = 0usize;
        loop {
            let before = i < pattern_tokens.len() && pattern_tokens[i].start_pos < m.from_pos;
            if before {
                i += 1;
                continue;
            }
            // `a && b || c`: the sentence start is tested even past the end.
            if skip_sentence_start && pattern_tokens.get(i)?.is_sent_start {
                i += 1;
                continue;
            }
            break;
        }
        let mut i = i as i64 + 1;
        if !offset.is_empty() {
            i += parse_int(offset)?;
        }
        i
    } else {
        parse_int(from)?
    };
    if position < 1 || position as usize > pattern_tokens.len() {
        return None;
    }
    Some(position as usize - 1)
}

/// A calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Today {
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
    pub day: u32,
}

/// What the filters of one engine share.
#[derive(Default)]
pub(crate) struct FilterEnv {
    analyzer: OnceLock<Arc<dyn SentenceAnalyzer>>,
    /// The rules filters may run on a corrected text.
    rules: OnceLock<Vec<PatternRule>>,
    english_tagger: OnceLock<Arc<dyn WordTagger>>,
    /// `year * 10000 + month * 100 + day`, 0 for the system clock.
    today: AtomicI64,
    /// Verdicts of `WordWithDeterminerFilter.suggestionHasNoErrors`.
    pub(crate) suggestion_verdicts: Mutex<HashMap<String, bool>>,
}

impl FilterEnv {
    /// `JLanguageTool.analyzeText(text).get(0)`.
    pub(crate) fn analyze(&self, text: &str, ctx: &RuleContext) -> Option<Sentence> {
        self.analyzer.get()?.analyze(text, ctx)
    }

    pub(crate) fn has_analyzer(&self) -> bool {
        self.analyzer.get().is_some()
    }

    /// The rules given by [`FrenchFilters::set_rules`].
    pub(crate) fn rules(&self) -> &[PatternRule] {
        self.rules.get().map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn english_tagger(&self) -> Option<&Arc<dyn WordTagger>> {
        self.english_tagger.get()
    }

    /// `Calendar.getInstance()`: the injected date, or the system's (UTC).
    pub(crate) fn today(&self) -> Today {
        let packed = self.today.load(Ordering::Relaxed);
        if packed != 0 {
            return Today {
                year: packed / 10000,
                month: (packed / 100 % 100) as u32,
                day: (packed % 100) as u32,
            };
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        dates::civil_from_days(secs.div_euclid(86_400))
    }
}

/// `JLanguageTool.ignoreRule` with no user choice: is the rule among
/// `getAllActiveRules()`?
fn is_active(rule: &PatternRule) -> bool {
    !(rule.info.category_default_off || rule.info.default_off || rule.info.default_temp_off)
}

/// The filters of the French rule files, and what they share.
#[derive(Clone, Default)]
pub struct FrenchFilters {
    env: Arc<FilterEnv>,
}

impl FrenchFilters {
    pub fn new() -> FrenchFilters {
        FrenchFilters::default()
    }

    /// The instance behind [`french_filter`].
    pub fn global() -> &'static FrenchFilters {
        static GLOBAL: OnceLock<FrenchFilters> = OnceLock::new();
        GLOBAL.get_or_init(FrenchFilters::new)
    }

    /// The filter for a Java class, named in full or by its last segment.
    /// `None` when the class has no port.
    pub fn filter(&self, class: &str) -> Option<Arc<dyn RuleFilter>> {
        let env = || self.env.clone();
        let filter: Arc<dyn RuleFilter> = match class.rsplit('.').next().unwrap_or(class) {
            "WordWithDeterminerFilter" => {
                Arc::new(determiner::WordWithDeterminerFilter::new(env()))
            }
            "PostponedAdjectiveConcordanceFilter" => {
                Arc::new(postponed_adjective::PostponedAdjectiveConcordanceFilter)
            }
            "SuggestionsFilter" => Arc::new(suggestions::SuggestionsFilter),
            "AdvancedSynthesizerFilter" => Arc::new(synthesizer::AdvancedSynthesizerFilter),
            "InterrogativeVerbFilter" => Arc::new(interrogative::InterrogativeVerbFilter),
            "DateCheckFilter" => Arc::new(dates::DateCheckFilter::new(env())),
            "NewYearDateFilter" => Arc::new(dates::NewYearDateFilter::new(env())),
            "FrenchNumberInWordFilter" => Arc::new(number_in_word::FrenchNumberInWordFilter),
            "MakeContractionsFilter" => Arc::new(contractions::MakeContractionsFilter),
            "SuppressIfAnyRuleMatchesFilter" => {
                Arc::new(suppress::SuppressIfAnyRuleMatchesFilter::new(env()))
            }
            "IsEnglishWordFilter" => Arc::new(english::IsEnglishWordFilter::new(env())),
            _ => return None,
        };
        Some(filter)
    }

    /// How filters analyse a text (`JLanguageTool.analyzeText`). Only the
    /// first call counts.
    pub fn set_analyzer(&self, analyzer: Arc<dyn SentenceAnalyzer>) {
        let _ = self.env.analyzer.set(analyzer);
    }

    /// Give the loaded grammar rules, for the filters that run rules on a
    /// corrected text (`JLanguageTool.getAllActiveRules()`): the active rules
    /// of the elision category, those `WordWithDeterminerFilter` names and
    /// those named by a `ruleIDs` filter argument are kept (as copies, which
    /// hold their own filters and so keep this object alive). Only the first
    /// call counts.
    pub fn set_rules(&self, rules: &[GrammarRule]) {
        let mut wanted: Vec<&str> = determiner::RULES_TO_CHECK.to_vec();
        for rule in rules {
            let GrammarRule::Pattern(p) = rule else {
                continue;
            };
            let Some(args) = p.rule.filter_args.as_deref() else {
                continue;
            };
            for arg in args.split_whitespace() {
                if let Some(ids) = arg.strip_prefix("ruleIDs:") {
                    wanted.extend(ids.split(','));
                }
            }
        }
        let kept: Vec<PatternRule> = rules
            .iter()
            .filter_map(|rule| match rule {
                GrammarRule::Pattern(p) => Some(p),
                GrammarRule::Regex(_) => None,
            })
            .filter(|p| {
                is_active(p)
                    && (p.info.category_id == determiner::CATEGORY_TO_CHECK
                        || wanted.contains(&p.info.id.as_str()))
            })
            .map(|p| PatternRule {
                info: p.info.clone(),
                rule: p.rule.clone(),
                anti_patterns: p.anti_patterns.clone(),
            })
            .collect();
        let _ = self.env.rules.set(kept);
    }

    /// The tagger `IsEnglishWordFilter` asks whether a word is English.
    /// Without one the filter rejects everything, as LanguageTool does when
    /// its English module is absent.
    pub fn set_english_tagger(&self, tagger: Arc<dyn WordTagger>) {
        let _ = self.env.english_tagger.set(tagger);
    }

    /// Fix the current date (tests); `None` goes back to the system clock.
    pub fn set_today(&self, today: Option<Today>) {
        let packed = today
            .map(|t| t.year * 10000 + t.month as i64 * 100 + t.day as i64)
            .unwrap_or(0);
        self.env.today.store(packed, Ordering::Relaxed);
    }
}

/// The filter factory of the French rule files, over [`FrenchFilters::global`].
pub fn french_filter(class: &str) -> Option<Arc<dyn RuleFilter>> {
    FrenchFilters::global().filter(class)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::engine::pattern::Lang;
    use crate::engine::synth::FrenchSynthesizer;
    use crate::engine::unifier::UnifierConfiguration;
    use std::path::Path;

    pub(crate) const ASSETS: &str = "assets/fr/lt";

    /// A context with the synthesizer and the stopgap tagger, when the
    /// resource files are there.
    pub(crate) fn context() -> Option<RuleContext> {
        let dir = Path::new(ASSETS);
        let mut lang = Lang::default();
        lang.synthesizer = Some(Arc::new(FrenchSynthesizer::load(dir).ok()?));
        lang.tagger = Some(Arc::new(SimpleDictTagger::load(dir).ok()?));
        Some(RuleContext {
            lang,
            unifier: UnifierConfiguration::default(),
            disambig_unifier: UnifierConfiguration::default(),
        })
    }

    pub(crate) fn bare_context() -> RuleContext {
        RuleContext {
            lang: Lang::default(),
            unifier: UnifierConfiguration::default(),
            disambig_unifier: UnifierConfiguration::default(),
        }
    }

    /// A sentence from `(token, [(tag, lemma)])` pairs, separated by spaces
    /// unless the previous token ends with an apostrophe.
    pub(crate) fn sentence(words: &[(&str, &[(&str, &str)])]) -> Sentence {
        let mut tokens = vec![Readings::new(
            vec![Reading::from_strs(
                "",
                Some(crate::engine::token::SENTENCE_START),
                None,
            )],
            0,
        )];
        let mut text = String::new();
        for (i, (word, readings)) in words.iter().enumerate() {
            let mut space: Option<Readings> = None;
            if i > 0 && !words[i - 1].0.ends_with('\'') {
                space = Some(Readings::untagged(" ", text.len()));
                text.push(' ');
            }
            let list: Vec<Reading> = if readings.is_empty() {
                vec![Reading::from_strs(word, None, None)]
            } else {
                readings
                    .iter()
                    .map(|(tag, lemma)| Reading::from_strs(word, Some(tag), Some(lemma)))
                    .collect()
            };
            let mut t = Readings::new(list, text.len());
            if let Some(sp) = space {
                t.set_whitespace_before(&sp.token.clone());
                tokens.push(sp);
            }
            text.push_str(word);
            tokens.push(t);
        }
        if let Some(last) = tokens.last_mut() {
            last.set_sent_end();
        }
        let pre = tokens.clone();
        Sentence::new(Arc::from(text.as_str()), tokens, pre)
    }

    /// A match over the non-whitespace tokens `from..=to` of `s`.
    pub(crate) fn match_over(s: &Sentence, from: usize, to: usize, message: &str) -> RuleMatch {
        RuleMatch::new(
            0,
            &s.text,
            s.tok(from).start_pos,
            s.tok(to).end_pos(),
            s.tok(from).start_pos,
            s.tok(to).end_pos(),
            message.to_string(),
            String::new(),
            false,
            false,
            "",
        )
    }

    pub(crate) fn args(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn factory_knows_the_ported_classes() {
        let f = FrenchFilters::new();
        for class in [
            "org.languagetool.rules.fr.WordWithDeterminerFilter",
            "PostponedAdjectiveConcordanceFilter",
            "org.languagetool.rules.fr.SuggestionsFilter",
            "AdvancedSynthesizerFilter",
            "InterrogativeVerbFilter",
            "DateCheckFilter",
            "NewYearDateFilter",
            "FrenchNumberInWordFilter",
            "MakeContractionsFilter",
            "org.languagetool.rules.SuppressIfAnyRuleMatchesFilter",
            "org.languagetool.rules.IsEnglishWordFilter",
        ] {
            assert!(f.filter(class).is_some(), "{class}");
        }
        // The speller-based filters are ported elsewhere.
        assert!(f
            .filter("org.languagetool.rules.fr.FindSuggestionsFilter")
            .is_none());
        assert!(french_filter("SuggestionsFilter").is_some());
    }

    #[test]
    fn tag_patterns_treat_untagged_readings_as_unknown() {
        let p = TagPattern::new("UNKNOWN|N .*");
        let unknown = Readings::untagged("xyz", 0);
        assert!(p.matches(&unknown));
        assert!(!p.matches_tagged(&unknown));
        let noun = Readings::new(
            vec![Reading::from_strs("chat", Some("N m s"), Some("chat"))],
            0,
        );
        assert!(p.matches(&noun) && p.matches_tagged(&noun));
        assert_eq!(p.find(&noun).and_then(|r| r.pos()), Some("N m s"));
        assert!(!TagPattern::new("J .*").matches(&noun));
    }

    #[test]
    fn marker_positions() {
        let s = sentence(&[("le", &[]), ("grand", &[]), ("chat", &[])]);
        let tokens = s.non_ws_tokens();
        let pattern_tokens = &tokens[1..];
        let m = match_over(&s, 2, 3, "");
        assert_eq!(token_position("marker", pattern_tokens, &m, false), Some(1));
        assert_eq!(
            token_position("marker1", pattern_tokens, &m, false),
            Some(2)
        );
        assert_eq!(
            token_position("marker-1", pattern_tokens, &m, true),
            Some(0)
        );
        assert_eq!(token_position("3", pattern_tokens, &m, false), Some(2));
        assert_eq!(token_position("4", pattern_tokens, &m, false), None);
        assert_eq!(token_position("marker2", pattern_tokens, &m, false), None);
    }

    #[test]
    fn today_can_be_fixed() {
        let f = FrenchFilters::new();
        f.set_today(Some(Today {
            year: 2014,
            month: 1,
            day: 15,
        }));
        assert_eq!(
            f.env.today(),
            Today {
                year: 2014,
                month: 1,
                day: 15
            }
        );
        f.set_today(None);
        assert!(f.env.today().year >= 2024);
    }
}
