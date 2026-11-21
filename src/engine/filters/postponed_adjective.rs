//! Port of `org.languagetool.rules.fr.PostponedAdjectiveConcordanceFilter`:
//! decides whether an adjective placed after a noun phrase disagrees with
//! every noun it may qualify, and proposes the agreeing forms.
//!
//! The filter walks back from the adjective over the noun phrase, counting
//! nouns and determiners by gender and number at each "level" (a level is
//! opened by a preposition: "les murs de la maison blanche").

use std::collections::HashMap;

use super::{tag_pattern, TagPattern};
use crate::engine::rule::{RuleContext, RuleFilter, RuleMatch};
use crate::engine::strtools;
use crate::engine::token::{Readings, Sentence};

const MAX_LEVELS: usize = 4;

tag_pattern!(nom, "[NZ] .*");
tag_pattern!(nom_ms, "[NZ] m s");
tag_pattern!(nom_fs, "[NZ] f s");
tag_pattern!(nom_mp, "[NZ] m p");
tag_pattern!(nom_mn, "[NZ] m sp");
tag_pattern!(nom_fp, "[NZ] f p");
tag_pattern!(nom_cs, "[NZ] e s");
tag_pattern!(nom_cp, "[NZ] e sp");
tag_pattern!(nom_det, "[NZ] .*|(P\\+)?D .*");
tag_pattern!(gn_any, "_GN_.*");
tag_pattern!(gn_tag_ms, "_GN_MS");
tag_pattern!(gn_tag_fs, "_GN_FS");
tag_pattern!(gn_tag_mp, "_GN_MP");
tag_pattern!(gn_tag_fp, "_GN_FP");
tag_pattern!(gn_tag_cs, "_GN_[MF]S");
tag_pattern!(gn_tag_cp, "_GN_[MF]P");
tag_pattern!(gn_tag_mn, "_GN_M[SP]");
tag_pattern!(gn_tag_fn, "_GN_F[SP]");
tag_pattern!(det, "(P\\+)?D .*");
tag_pattern!(det_cs, "(P\\+)?D e s");
tag_pattern!(det_ms, "(P\\+)?D m s");
tag_pattern!(det_fs, "(P\\+)?D f s");
tag_pattern!(det_mp, "(P\\+)?D m p");
tag_pattern!(det_fp, "(P\\+)?D f p");
tag_pattern!(det_cp, "(P\\+)?D e p");
tag_pattern!(
    gn_ms,
    "[NZ] [me] (s|sp)|J [me] (s|sp)|V ppa m s|(P\\+)?D m (s|sp)"
);
tag_pattern!(
    gn_fs,
    "[NZ] [fe] (s|sp)|J [fe] (s|sp)|V ppa f s|(P\\+)?D f (s|sp)"
);
tag_pattern!(
    gn_mp,
    "[NZ] [me] (p|sp)|J [me] (p|sp)|V ppa m p|(P\\+)?D m (p|sp)"
);
tag_pattern!(
    gn_fp,
    "[NZ] [fe] (p|sp)|J [fe] (p|sp)|V ppa f p|(P\\+)?D f (p|sp)"
);
tag_pattern!(
    gn_cp,
    "[NZ] [fme] (p|sp)|J [fme] (p|sp)|(P\\+)?D [fme] (p|sp)"
);
tag_pattern!(
    gn_cs,
    "[NZ] [fme] (s|sp)|J [fme] (s|sp)|(P\\+)?D [fme] (s|sp)"
);
tag_pattern!(
    gn_mn,
    "[NZ] [me] (s|p|sp)|J [me] (s|p|sp)|(P\\+)?D [me] (s|p|sp)"
);
tag_pattern!(
    gn_fn,
    "[NZ] [fe] (s|p|sp)|J [fe] (s|p|sp)|(P\\+)?D [fe] (s|p|sp)"
);
tag_pattern!(adjectif, "J .*|V ppa .*|PX.*");
tag_pattern!(adjectif_ms, "J [me] (s|sp)|V ppa m s");
tag_pattern!(adjectif_fs, "J [fe] (s|sp)|V ppa f s");
tag_pattern!(adjectif_mp, "J [me] (p|sp)|V ppa m p");
tag_pattern!(adjectif_fp, "J [fe] (p|sp)|V ppa f p");
tag_pattern!(adjectif_cp, "J e (p|sp)");
tag_pattern!(adjectif_cs, "J e (s|sp)");
tag_pattern!(adjectif_mn, "J m sp");
tag_pattern!(adjectif_fn, "J f sp");
tag_pattern!(adjectif_s, "J .* (s|sp)|V ppa . s");
tag_pattern!(adjectif_p, "J .* (p|sp)|V ppa . p");
tag_pattern!(adjectif_m, "J [me] .*|V ppa [me] .*");
tag_pattern!(adverbe, "A");
tag_pattern!(conjonction, "C .*");
tag_pattern!(ponctuation, "_PUNCT");
tag_pattern!(
    keep_count,
    "Y|J .*|N .*|D .*|P.*|V ppa .*|M nonfin|UNKNOWN|Z.*|V.* inf|V ppr"
);
tag_pattern!(prepositions, "P.*");
tag_pattern!(verbe, "V.* (inf|ind|sub|con|ppr|imp).*");
tag_pattern!(infinitif, "V.* inf");
tag_pattern!(gv, "_GV_");

/// `COORDINACIO_IONI`.
fn is_coordination(token: &str) -> bool {
    matches!(token, "et" | "ou" | "ni")
}

/// `KEEP_COUNT2`.
fn is_keep_count2(token: &str) -> bool {
    matches!(token, "," | "et" | "ou" | "ni")
}

/// `STOP_COUNT`.
fn is_stop_count(token: &str) -> bool {
    matches!(
        token,
        ";" | ":" | "(" | ")" | "[" | "]" | "–" | "—" | "―" | "‒"
    )
}

/// `PREPOSICIO_CANVI_NIVELL`: the prepositions that open a level.
fn is_level_preposition(token: &str) -> bool {
    matches!(
        token,
        "d'" | "de"
            | "des"
            | "du"
            | "à"
            | "au"
            | "aux"
            | "en"
            | "dans"
            | "sur"
            | "entre"
            | "par"
            | "pour"
            | "avec"
            | "sans"
            | "contre"
            | "comme"
    )
}

/// What has been seen since the last noun or adjective while walking back.
#[derive(Default)]
struct Apparitions {
    adverb: bool,
    conjunction: bool,
    punctuation: bool,
    infinitive: bool,
}

impl Apparitions {
    /// `updateApparitions`.
    fn update(&mut self, t: &Readings) {
        self.conjunction |= conjonction().matches(t);
        if &*t.token == "com" {
            return;
        }
        if nom().matches(t) || adjectif().matches(t) {
            *self = Apparitions::default();
            return;
        }
        self.adverb |= adverbe().matches(t);
        self.punctuation |= ponctuation().matches(t) || &*t.token == ",";
        self.infinitive |= infinitif().matches(t);
    }

    /// `keepCounting`: does the noun phrase go on with this token?
    fn keep_counting(&self, t: &Readings) -> bool {
        let token: &str = &t.token;
        if is_level_preposition(token) {
            return true;
        }
        // Not the end of the sentence, but an abbreviation.
        if token == "." {
            return true;
        }
        if (self.adverb && self.conjunction)
            || (self.adverb && self.punctuation)
            || (self.conjunction && self.punctuation)
            || (self.punctuation && ponctuation().matches(t))
            || (self.infinitive && is_coordination(token))
            || (self.infinitive && self.adverb)
        {
            return false;
        }
        (keep_count().matches(t) || is_keep_count2(token) || adverbe().matches(t))
            && !is_stop_count(token)
            && (!gv().matches(t) || gn_any().matches(t))
    }
}

/// `updateJValue`: "deux ou plus" is stepped over.
fn update_j(tokens: &[&Readings], i: usize, j: usize) -> usize {
    if is_coordination(&tokens[i - j].token)
        && i > j + 1
        && i - j + 1 < tokens.len()
        && det().matches(tokens[i - j - 1])
        && &*tokens[i - j + 1].token == "plus"
    {
        return j + 1;
    }
    j
}

pub(crate) struct PostponedAdjectiveConcordanceFilter;

impl RuleFilter for PostponedAdjectiveConcordanceFilter {
    fn accept(
        &self,
        mut m: RuleMatch,
        _args: &HashMap<String, String>,
        pattern_token_pos: usize,
        _pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        sentence: &Sentence,
        ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        let tokens: Vec<&Readings> = sentence.non_ws_tokens();
        let i = pattern_token_pos;
        if i == 0 || i >= tokens.len() {
            return None;
        }
        let adjective = tokens[i];
        let mut is_plural = true;
        let mut is_prev_noun = false;
        let (mut can_be_ms, mut can_be_fs, mut can_be_mp, mut can_be_fp, mut can_be_p) =
            (false, false, false, false, false);
        // Counts per level: nouns in all, nouns by gender and number,
        // determiners by gender and number.
        let mut c_nt = [0i32; MAX_LEVELS];
        let mut c_nms = [0i32; MAX_LEVELS];
        let mut c_nfs = [0i32; MAX_LEVELS];
        let mut c_nmp = [0i32; MAX_LEVELS];
        let mut c_nmn = [0i32; MAX_LEVELS];
        let mut c_nfp = [0i32; MAX_LEVELS];
        let mut c_ncs = [0i32; MAX_LEVELS];
        let mut c_ncp = [0i32; MAX_LEVELS];
        let mut c_dms = [0i32; MAX_LEVELS];
        let mut c_dfs = [0i32; MAX_LEVELS];
        let mut c_dmp = [0i32; MAX_LEVELS];
        let mut c_dfp = [0i32; MAX_LEVELS];
        let mut level = 0usize;
        let mut j = 1usize;
        let mut seen = Apparitions::default();
        while i > j && seen.keep_counting(tokens[i - j]) && level < MAX_LEVELS {
            let t = tokens[i - j];
            let next = tokens[i - j + 1];
            if !is_prev_noun {
                if nom().matches(t)
                    || (i > j + 1
                        && !nom().matches(t)
                        && adjectif().matches(t)
                        && det().matches(tokens[i - j - 1]))
                {
                    if gn_tag_ms().matches(t) {
                        c_nms[level] += 1;
                        can_be_ms = true;
                    }
                    if gn_tag_fs().matches(t) {
                        c_nfs[level] += 1;
                        can_be_fs = true;
                    }
                    if gn_tag_mp().matches(t) {
                        c_nmp[level] += 1;
                        can_be_mp = true;
                    }
                    if gn_tag_fp().matches(t) {
                        c_nfp[level] += 1;
                        can_be_fp = true;
                    }
                }
                if !gn_any().matches(t) {
                    if nom_ms().matches(t) {
                        c_nms[level] += 1;
                        can_be_ms = true;
                    } else if nom_fs().matches(t) {
                        c_nfs[level] += 1;
                        can_be_fs = true;
                    } else if nom_mp().matches(t) {
                        c_nmp[level] += 1;
                        can_be_mp = true;
                    } else if nom_mn().matches(t) {
                        c_nmn[level] += 1;
                        can_be_ms = true;
                        can_be_mp = true;
                    } else if nom_fp().matches(t) {
                        c_nfp[level] += 1;
                        can_be_fp = true;
                    } else if nom_cs().matches(t) {
                        c_ncs[level] += 1;
                        can_be_ms = true;
                        can_be_fs = true;
                    } else if nom_cp().matches(t) {
                        c_ncp[level] += 1;
                        can_be_fp = true;
                        can_be_mp = true;
                    }
                }
            }
            if nom().matches(t) {
                c_nt[level] += 1;
                is_prev_noun = true;
            } else {
                is_prev_noun = false;
            }
            // An epicene determiner takes the gender of the noun after it.
            if det_cs().matches(t) {
                if nom_ms().matches(next) {
                    c_dms[level] += 1;
                    can_be_ms = true;
                }
                if nom_fs().matches(next) {
                    c_dfs[level] += 1;
                    can_be_fs = true;
                }
            }
            if det_cp().matches(t) {
                // As in LanguageTool, plural nouns are counted with the
                // singular determiners here.
                if nom_mp().matches(next) {
                    c_dms[level] += 1;
                    can_be_mp = true;
                }
                if nom_fp().matches(next) {
                    c_dfs[level] += 1;
                    can_be_fp = true;
                }
            }
            if !adverbe().matches(t) {
                if det_ms().matches(t) {
                    c_dms[level] += 1;
                    can_be_ms = true;
                }
                if det_fs().matches(t) {
                    c_dfs[level] += 1;
                    can_be_fs = true;
                }
                if det_mp().matches(t) {
                    c_dmp[level] += 1;
                    can_be_mp = true;
                }
                if det_fp().matches(t) {
                    c_dfp[level] += 1;
                    can_be_fp = true;
                }
            }
            // A preposition opens a level ("des" only when it is one), and
            // so does "d'environ".
            if i > j + 1
                && ((is_level_preposition(&t.token)
                    && prepositions().matches(t)
                    && !conjonction().matches(t)
                    && !is_coordination(&tokens[i - j - 1].token)
                    && !adverbe().matches(next))
                    || (strtools::equals_ignore_case(&t.token, "d'")
                        && strtools::equals_ignore_case(&next.token, "environ")))
            {
                level += 1;
            }
            j = update_j(&tokens, i, j);
            seen.update(tokens[i - j]);
            j += 1;
        }
        level = (level + 1).min(MAX_LEVELS);
        let mut c_n_total = 0;
        let mut c_d_total = 0;
        for l in 0..level {
            let c_n = c_nms[l] + c_nfs[l] + c_nmp[l] + c_nfp[l] + c_ncs[l] + c_ncp[l] + c_nmn[l];
            let c_d = c_dms[l] + c_dfs[l] + c_dmp[l] + c_dfp[l];
            c_n_total += c_n;
            c_d_total += c_d;
            // Adjective in the masculine plural: is there a masculine among
            // several nouns?
            if adjectif_mp().matches(adjective)
                && (c_n > 1 || c_d > 1)
                && (c_nms[l] + c_nmn[l] + c_nmp[l] + c_ncs[l] + c_ncp[l] + c_dms[l] + c_dmp[l]) > 0
                && (c_nfs[l] + c_nfp[l] <= c_nt[l])
            {
                return None;
            }
            // Adjective in the feminine plural: are all the nouns feminine?
            if adjectif_fp().matches(adjective)
                && (c_n > 1 || c_d > 1)
                && ((c_nms[l] + c_nmp[l] + c_nmn[l] + c_dms[l] + c_dmp[l]) == 0
                    || (c_nt[l] > 0 && c_nfs[l] + c_nfp[l] >= c_nt[l]))
            {
                return None;
            }
            if c_n + c_d > 0 {
                is_plural = is_plural && c_d > 1 && level > 1;
                can_be_p = can_be_p || c_n > 1;
            }
        }
        is_plural = is_plural
            || (i > 2 && c_nmp[0] + c_nfp[0] + c_ncp[0] > 0 && &*tokens[i - 2].token == ",");
        // There is neither noun nor determiner: the adjective is on its own.
        if c_n_total == 0 && c_d_total == 0 {
            return None;
        }

        // What the adjective would agree with: (noun or determiner, nominal
        // group tag, adjective).
        let (subst, gn, adj): (&TagPattern, &TagPattern, &TagPattern) =
            if adjectif_cs().matches(adjective) {
                (gn_cs(), gn_tag_cs(), adjectif_s())
            } else if adjectif_cp().matches(adjective) {
                (gn_cp(), gn_tag_cp(), adjectif_p())
            } else if adjectif_mn().matches(adjective) {
                (gn_mn(), gn_tag_mn(), adjectif_m())
            } else if adjectif_fn().matches(adjective) {
                (gn_fn(), gn_tag_fn(), adjectif_fn())
            } else if adjectif_ms().matches(adjective) {
                (gn_ms(), gn_tag_ms(), adjectif_ms())
            } else if adjectif_fs().matches(adjective) {
                (gn_fs(), gn_tag_fs(), adjectif_fs())
            } else if adjectif_mp().matches(adjective) {
                (gn_mp(), gn_tag_mp(), adjectif_mp())
            } else if adjectif_fp().matches(adjective) {
                (gn_fp(), gn_tag_fp(), adjectif_fp())
            } else {
                return None;
            };

        // Combinations noun (+ adjectives) + adjective that agree.
        j = 1;
        let mut keep_count = true;
        while i > j && keep_count {
            let t = tokens[i - j];
            if (nom_det().matches(t) && gn.matches(t)) || (!gn_any().matches(t) && subst.matches(t))
            {
                return None;
            }
            keep_count = !nom_det().matches(t);
            j += 1;
        }

        // The word before the adjective must leave no agreement possible.
        let prev = tokens[i - 1];
        let no_agreement = (nom().matches(prev) && !subst.matches(prev))
            || (gn_any().matches(prev) && !gn.matches(prev))
            || (adjectif().matches(prev) && !adj.matches(prev))
            || (i > 2
                && adverbe().matches(prev)
                && !verbe().matches(tokens[i - 2])
                && !prepositions().matches(tokens[i - 2]))
            || (i > 3
                && adverbe().matches(prev)
                && adverbe().matches(tokens[i - 2])
                && !verbe().matches(tokens[i - 3])
                && !prepositions().matches(tokens[i - 3]));
        if !no_agreement {
            return None;
        }

        // Unless the adjective cannot be singular, look for an agreeing
        // noun or nominal group among the previous words.
        if !(is_plural && adjectif_s().matches(adjective)) {
            j = 1;
            seen = Apparitions::default();
            while i > j && seen.keep_counting(tokens[i - j]) && (level > 1 || j < 4) {
                let t = tokens[i - j];
                if (!gn_any().matches(t) && nom_det().matches(t) && subst.matches(t))
                    || gn.matches(t)
                {
                    return None;
                }
                j = update_j(&tokens, i, j);
                seen.update(tokens[i - j]);
                j += 1;
            }
        }

        // The rule matches: synthesize the suggestions.
        let synth = ctx.lang.synthesizer.as_ref()?;
        let mut suggestions: Vec<String> = Vec::new();
        if let Some(at) = adjectif_cs().find(adjective) {
            suggestions.extend(synth.synthesize_regexp(at, "J e p", true));
        }
        if suggestions.is_empty() {
            if let Some(at) = adjectif_cp().find(adjective) {
                suggestions.extend(synth.synthesize_regexp(at, "J e s", true));
            }
        }
        if suggestions.is_empty() && is_plural {
            if let Some(at) = adjectif_p().find(adjective) {
                suggestions.extend(synth.synthesize_regexp(at, "J . p|V ppa . p", true));
            }
        }
        if let Some(at) = adjectif()
            .find(adjective)
            .filter(|_| suggestions.is_empty())
        {
            if can_be_ms && !is_plural {
                suggestions.extend(synth.synthesize_regexp(at, "J [me] sp?|V ppa m s", true));
            }
            if can_be_fs && !is_plural {
                suggestions.extend(synth.synthesize_regexp(at, "J [fe] sp?|V ppa f s", true));
            }
            if can_be_mp {
                suggestions.extend(synth.synthesize_regexp(at, "J [me] s?p|V ppa m p", true));
            }
            if can_be_fp {
                suggestions.extend(synth.synthesize_regexp(at, "J [fe] s?p|V ppa f p", true));
            }
            if can_be_ms && (is_plural || can_be_p) {
                suggestions.extend(synth.synthesize_regexp(at, "J [me] s?p|V ppa m p", true));
            }
            if can_be_fs && !can_be_ms && (is_plural || can_be_p) {
                suggestions.extend(synth.synthesize_regexp(at, "J [fe] s?p|V ppa f p", true));
            }
        }
        let mut distinct: Vec<String> = Vec::with_capacity(suggestions.len());
        for s in suggestions {
            if !distinct.contains(&s) {
                distinct.push(s);
            }
        }
        // The original word is not a suggestion.
        let original = adjective.token.to_lowercase();
        distinct.retain(|s| *s != original);
        if distinct.is_empty() {
            return None;
        }
        m.suggestions = distinct;
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;

    type Word<'a> = (&'a str, &'a [(&'a str, &'a str)]);

    fn run(words: &[Word], adjective: usize) -> Option<Vec<String>> {
        let ctx = context()?;
        let s = sentence(words);
        let tokens = s.non_ws_tokens();
        let m = match_over(&s, adjective, adjective, "");
        Some(
            PostponedAdjectiveConcordanceFilter
                .accept(
                    m,
                    &args(&[("key", "value")]),
                    adjective,
                    &tokens[adjective..=adjective],
                    &[1],
                    &s,
                    &ctx,
                )
                .map(|m| m.suggestions)
                .unwrap_or_default(),
        )
    }

    const UNE: Word = ("une", &[("D f s", "un")]);
    const MAISON: Word = ("maison", &[("N f s", "maison")]);
    const BLANC: Word = ("blanc", &[("J m s", "blanc")]);
    const BLANCHE: Word = ("blanche", &[("J f s", "blanc")]);

    #[test]
    fn a_disagreeing_adjective_gets_the_agreeing_forms() {
        let Some(found) = run(&[UNE, MAISON, BLANC], 3) else {
            return;
        };
        assert_eq!(found, ["blanche"]);
    }

    #[test]
    fn an_agreeing_adjective_is_accepted() {
        let Some(found) = run(&[UNE, MAISON, BLANCHE], 3) else {
            return;
        };
        assert!(found.is_empty());
    }

    #[test]
    fn a_noun_of_a_deeper_level_may_carry_the_agreement() {
        // "le mur de la maison blanche": "blanche" agrees with "maison";
        // "blanc" agrees with "mur".
        let le: Word = ("le", &[("D m s", "le")]);
        let mur: Word = ("mur", &[("N m s", "mur")]);
        let de: Word = ("de", &[("P", "de")]);
        let la: Word = ("la", &[("D f s", "le")]);
        let Some(found) = run(&[le, mur, de, la, MAISON, BLANC], 6) else {
            return;
        };
        assert!(found.is_empty(), "{found:?}");
        // "le mur de la maison blanches" agrees with nothing.
        let blanches: Word = ("blanches", &[("J f p", "blanc")]);
        let Some(found) = run(&[le, mur, de, la, MAISON, blanches], 6) else {
            return;
        };
        assert_eq!(found, ["blanc", "blanche"]);
    }

    #[test]
    fn nothing_to_agree_with_means_no_match() {
        let il: Word = ("il", &[("R pers suj 3 m s", "il")]);
        let Some(found) = run(&[il, BLANC], 2) else {
            return;
        };
        assert!(found.is_empty());
    }
}
