//! Port of `org.languagetool.rules.SuppressIfAnyRuleMatchesFilter`: drops a
//! match when, once one of its suggestions is applied, one of the rules
//! named in `ruleIDs` complains about the corrected place.
//!
//! The named rules and the analysis of the corrected sentence come from the
//! filter's [`FrenchFilters`](super::FrenchFilters) (`set_rules`,
//! `set_analyzer`). A rule id nobody loaded matches nothing, as in
//! LanguageTool.

use std::collections::HashMap;
use std::sync::Arc;

use super::FilterEnv;
use crate::engine::rule::{PatternRule, RuleContext, RuleFilter, RuleMatch};
use crate::engine::strtools::utf16_len;
use crate::engine::token::{Readings, Sentence};

pub(crate) struct SuppressIfAnyRuleMatchesFilter {
    env: Arc<FilterEnv>,
}

impl SuppressIfAnyRuleMatchesFilter {
    pub(crate) fn new(env: Arc<FilterEnv>) -> SuppressIfAnyRuleMatchesFilter {
        SuppressIfAnyRuleMatchesFilter { env }
    }
}

/// `anyRuleMatches`: a match of one of the rules touching the end of the
/// original match.
fn any_rule_matches(rules: &[&PatternRule], sentence: &Sentence, m: &RuleMatch, ctx: &RuleContext) -> bool {
    rules.iter().any(|rule| {
        rule.match_sentence(usize::MAX, sentence, ctx)
            .iter()
            .any(|o| (o.to_pos >= m.from_pos && o.to_pos <= m.to_pos) || (m.to_pos >= o.from_pos && m.to_pos <= o.to_pos))
    })
}

/// `replaceTokenInAnalyzedSentence`: the sentence with the matched token
/// retagged as `replacement`, when the match is one token and the
/// replacement has its length.
fn replace_token(sentence: &Sentence, m: &RuleMatch, replacement: &str, ctx: &RuleContext) -> Option<Sentence> {
    let original = sentence.text.get(m.from_pos..m.to_pos)?;
    if utf16_len(replacement) != utf16_len(original) || replacement.contains(' ') {
        return None;
    }
    let idx = sentence.tokens.iter().position(|t| !t.is_whitespace && t.start_pos == m.from_pos)?;
    let tagged = ctx.lang.tagger.as_ref()?.tag_word(replacement);
    let mut tokens = sentence.tokens.clone();
    tokens[idx] = Readings::with_readings(&sentence.tokens[idx], tagged.readings);
    let mut pre_disambig = sentence.pre_disambig.clone();
    if idx < pre_disambig.len() {
        pre_disambig[idx] = tokens[idx].clone();
    }
    let text: String = tokens.iter().map(|t| &*t.token).collect();
    Some(Sentence::new(Arc::from(text.as_str()), tokens, pre_disambig))
}

impl RuleFilter for SuppressIfAnyRuleMatchesFilter {
    fn accept(
        &self,
        m: RuleMatch,
        args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        _pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        sentence: &Sentence,
        ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        let rule_ids: Vec<&str> = args.get("ruleIDs")?.split(',').collect();
        let fast_only = args.get("fastOnly").is_some_and(|v| v == "true");
        let rules: Vec<&PatternRule> = self.env.rules().iter().filter(|r| rule_ids.contains(&r.info.id.as_str())).collect();
        if rules.is_empty() {
            return Some(m);
        }
        for replacement in &m.suggestions {
            if let Some(partially_analyzed) = replace_token(sentence, &m, replacement, ctx) {
                if any_rule_matches(&rules, &partially_analyzed, &m, ctx) {
                    return None;
                }
                if fast_only {
                    continue;
                }
            }
            let new_sentence = format!("{}{}{}", &sentence.text[..m.from_pos], replacement, &sentence.text[m.to_pos..]);
            if let Some(analyzed) = self.env.analyze(&new_sentence, ctx) {
                if any_rule_matches(&rules, &analyzed, &m, ctx) {
                    return None;
                }
            }
        }
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;
    use crate::engine::filters::FrenchFilters;
    use crate::engine::unifier::UnifierConfiguration;
    use crate::engine::xmlrules::load_grammar;

    const RULES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rules lang="fr">
  <category id="TEST" name="test">
    <rule id="NO_OU" name="ou">
      <pattern><token>ou</token></pattern>
      <message>Pas de « ou ».</message>
    </rule>
  </category>
</rules>"#;

    #[test]
    fn a_suggestion_another_rule_rejects_suppresses_the_match() {
        let Some(ctx) = context() else { return };
        let filters = FrenchFilters::new();
        let no_filters = |_: &str| None;
        let mut unifier = UnifierConfiguration::default();
        let mut rules = load_grammar(RULES, "test.xml", false, &no_filters, &mut unifier);
        // Declare that some rule's filter names NO_OU, as `set_rules` expects.
        if let Some(crate::engine::xmlrules::GrammarRule::Pattern(p)) = rules.first_mut() {
            p.rule.filter_args = Some("ruleIDs:NO_OU".to_string());
        }
        filters.set_rules(&rules);
        let filter = SuppressIfAnyRuleMatchesFilter::new(filters.env.clone());

        let s = sentence(&[("le", &[("D m s", "le")]), ("où", &[("A", "où")]), ("là", &[("A", "là")])]);
        let tokens = s.non_ws_tokens();
        let mut m = match_over(&s, 2, 2, "");
        m.suggestions = vec!["ou".into()];
        let accept = |m: RuleMatch, ids: &str| filter.accept(m, &args(&[("ruleIDs", ids)]), 2, &tokens[2..=2], &[1], &s, &ctx);
        assert!(accept(m.clone(), "NO_OU").is_none(), "NO_OU matches the corrected token");
        assert!(accept(m.clone(), "UNKNOWN_RULE,OTHER").is_some(), "unknown rules match nothing");
        m.suggestions = vec!["et".into()];
        assert!(accept(m, "NO_OU").is_some());
    }
}
