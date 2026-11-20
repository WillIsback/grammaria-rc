//! Feature unification (`<unify>`): do several tokens agree on gender, number,
//! person...?
//!
//! Port of `org.languagetool.rules.patterns.Unifier` and
//! `UnifierConfiguration` (LanguageTool, LGPL-2.1-or-later). The algorithm is
//! stateful and its bookkeeping maps are shared between two views, exactly as
//! in the original; it is kept line for line rather than re-imagined.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use super::pattern::{PatternToken, UniFeatures};
use super::token::{Reading, Readings};

const UNIFY_IGNORE: &str = "unify-ignore";

/// The `<unification>` definitions of a rule file: for each feature
/// (`number`) and equivalence type (`sg`), the token description that a
/// reading must satisfy to have that type.
#[derive(Clone, Debug, Default)]
pub struct UnifierConfiguration {
    types: HashMap<(String, String), Arc<PatternToken>>,
    features: HashMap<String, Vec<String>>,
}

impl UnifierConfiguration {
    /// `setEquivalence`: the first definition of a (feature, type) wins.
    pub fn set_equivalence(&mut self, feature: &str, type_name: &str, elem: PatternToken) {
        let key = (feature.to_string(), type_name.to_string());
        if self.types.contains_key(&key) {
            return;
        }
        self.types.insert(key, Arc::new(elem));
        self.features.entry(feature.to_string()).or_default().push(type_name.to_string());
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn create_unifier(&self) -> Unifier<'_> {
        Unifier::new(self)
    }
}

type Equiv = Rc<RefCell<HashMap<String, HashSet<String>>>>;

pub struct Unifier<'c> {
    config: &'c UnifierConfiguration,
    tok_sequence: Vec<Readings>,
    tok_sequence_equivalences: Vec<Vec<Equiv>>,
    equivalences_matched: Vec<Equiv>,
    all_feats_in: bool,
    tok_cnt: usize,
    readings_counter: usize,
    features_found: Vec<bool>,
    tmp_features_found: Vec<bool>,
    equivalences_to_be_kept: HashMap<String, HashSet<String>>,
    unification_feats: Option<Arc<UniFeatures>>,
    in_unification: bool,
    uni_matched: bool,
    uni_all_matched: bool,
}

impl<'c> Unifier<'c> {
    pub fn new(config: &'c UnifierConfiguration) -> Unifier<'c> {
        Unifier {
            config,
            tok_sequence: Vec::new(),
            tok_sequence_equivalences: Vec::new(),
            equivalences_matched: Vec::new(),
            all_feats_in: false,
            tok_cnt: 0,
            readings_counter: 1,
            features_found: Vec::new(),
            tmp_features_found: Vec::new(),
            equivalences_to_be_kept: HashMap::new(),
            unification_feats: None,
            in_unification: false,
            uni_matched: false,
            uni_all_matched: false,
        }
    }

    fn types_of<'a>(&'a self, feature: &str, listed: &'a [String]) -> &'a [String] {
        if listed.is_empty() {
            self.config.features.get(feature).map(Vec::as_slice).unwrap_or(&[])
        } else {
            listed
        }
    }

    fn is_satisfied(&mut self, token: &Reading, features: &Arc<UniFeatures>) -> bool {
        if self.all_feats_in && self.equivalences_matched.is_empty() {
            return false;
        }
        self.unification_feats = Some(features.clone());
        let mut unified = true;
        if self.all_feats_in {
            unified = self.check_next(token, features);
        } else {
            while self.equivalences_matched.len() <= self.tok_cnt {
                self.equivalences_matched.push(Rc::new(RefCell::new(HashMap::new())));
            }
            for (feature, listed) in features.iter() {
                let types: Vec<String> = self.types_of(feature, listed).to_vec();
                for type_name in &types {
                    let Some(test_elem) = self.config.types.get(&(feature.clone(), type_name.clone())) else {
                        return false;
                    };
                    if test_elem.is_matched(token) {
                        self.equivalences_matched[self.tok_cnt]
                            .borrow_mut()
                            .entry(feature.clone())
                            .or_default()
                            .insert(type_name.clone());
                    }
                }
                unified = self.equivalences_matched[self.tok_cnt].borrow().contains_key(feature);
                if !unified {
                    self.equivalences_matched.remove(self.tok_cnt);
                    break;
                }
            }
            if unified {
                let equiv = self.equivalences_matched[self.tok_cnt].clone();
                if self.tok_cnt == 0 || self.tok_sequence.is_empty() {
                    self.tok_sequence.push(Readings::new(vec![token.clone()], 0));
                    self.tok_sequence_equivalences.push(vec![equiv]);
                } else {
                    self.tok_sequence[0].add_reading(token.clone());
                    self.tok_sequence_equivalences[0].push(equiv);
                }
                self.tok_cnt += 1;
            }
        }
        unified
    }

    fn check_next(&mut self, token: &Reading, features: &Arc<UniFeatures>) -> bool {
        let mut any_feat_unified = false;
        let mut token_features_found = self.tmp_features_found.clone();
        let here: Equiv = Rc::new(RefCell::new(HashMap::new()));
        if self.all_feats_in {
            for i in 0..self.tok_cnt {
                let mut all_feats_unified = true;
                for (feature, listed) in features.iter() {
                    let mut feat_unified = false;
                    let types: Vec<String> = self.types_of(feature, listed).to_vec();
                    for type_name in &types {
                        let present = self
                            .equivalences_matched
                            .get(i)
                            .is_some_and(|m| m.borrow().get(feature).is_some_and(|s| s.contains(type_name)));
                        if present {
                            let matched = self
                                .config
                                .types
                                .get(&(feature.clone(), type_name.clone()))
                                .is_some_and(|e| e.is_matched(token));
                            feat_unified = feat_unified || matched;
                            if matched {
                                self.equivalences_to_be_kept.entry(feature.clone()).or_default().insert(type_name.clone());
                                here.borrow_mut().entry(feature.clone()).or_default().insert(type_name.clone());
                            }
                        }
                    }
                    all_feats_unified &= feat_unified;
                }
                if let Some(slot) = token_features_found.get_mut(i) {
                    *slot = *slot || all_feats_unified;
                }
                any_feat_unified = any_feat_unified || all_feats_unified;
            }
            if any_feat_unified {
                if self.tok_sequence.len() == self.readings_counter {
                    self.tok_sequence.push(Readings::new(vec![token.clone()], 0));
                    self.tok_sequence_equivalences.push(vec![here]);
                } else if self.readings_counter < self.tok_sequence.len() {
                    self.tok_sequence[self.readings_counter].add_reading(token.clone());
                    self.tok_sequence_equivalences[self.readings_counter].push(here);
                } else {
                    any_feat_unified = false;
                }
                self.tmp_features_found = token_features_found;
            }
        }
        any_feat_unified
    }

    /// `startNextToken`: narrow the recorded equivalences to those the token
    /// just processed kept alive.
    pub fn start_next_token(&mut self) {
        self.features_found = self.tmp_features_found.clone();
        self.readings_counter += 1;
        for j in 0..self.tok_sequence.len() {
            for i in 0..self.tok_sequence_equivalences[j].len() {
                for feature in self.config.features.keys() {
                    if feature == UNIFY_IGNORE {
                        continue;
                    }
                    let mut map = self.tok_sequence_equivalences[j][i].borrow_mut();
                    if map.contains_key(feature) {
                        match self.equivalences_to_be_kept.get(feature) {
                            Some(keep) => {
                                if let Some(set) = map.get_mut(feature) {
                                    set.retain(|t| keep.contains(t));
                                }
                            }
                            None => {
                                map.remove(feature);
                            }
                        }
                    } else {
                        map.remove(feature);
                    }
                }
            }
        }
        self.equivalences_to_be_kept.clear();
    }

    pub fn start_unify(&mut self) {
        self.all_feats_in = true;
        for _ in 0..self.tok_cnt {
            self.features_found.push(false);
        }
        self.tmp_features_found = self.features_found.clone();
    }

    pub fn final_unification_value(&self, features: &UniFeatures) -> bool {
        let wanted = self.unification_feats.as_ref().map(|f| f.len()).unwrap_or(0);
        let mut tok_unified = 0usize;
        for j in 0..self.tok_sequence.len() {
            let mut unified_tokens_found = false;
            for (i, equiv) in self.tok_sequence_equivalences[j].iter().enumerate() {
                let map = equiv.borrow();
                let mut feat_unified = 0usize;
                if map.contains_key(UNIFY_IGNORE) {
                    if i == 0 {
                        tok_unified += 1;
                    }
                    unified_tokens_found = true;
                    continue;
                }
                for feature in features.keys() {
                    if map.get(feature).is_some_and(|s| s.is_empty()) {
                        feat_unified = 0;
                    } else {
                        feat_unified += 1;
                    }
                    if feat_unified == wanted && tok_unified <= j {
                        tok_unified += 1;
                        unified_tokens_found = true;
                        break;
                    }
                }
            }
            if !unified_tokens_found {
                return false;
            }
        }
        tok_unified == self.tok_sequence.len()
    }

    pub fn reset(&mut self) {
        self.equivalences_matched.clear();
        self.all_feats_in = false;
        self.tok_cnt = 0;
        self.features_found.clear();
        self.tmp_features_found.clear();
        self.tok_sequence.clear();
        self.tok_sequence_equivalences.clear();
        self.readings_counter = 1;
        self.uni_matched = false;
        self.uni_all_matched = false;
        self.in_unification = false;
    }

    /// `getUnifiedTokens`: the tokens restricted to their unified readings.
    pub fn unified_tokens(&self) -> Option<Vec<Readings>> {
        if self.tok_sequence.is_empty() {
            return None;
        }
        let feats = self.unification_feats.as_ref()?;
        let mut out: Vec<Readings> = Vec::new();
        for j in 0..self.tok_sequence.len() {
            let mut unified_tokens_found = false;
            for (i, equiv) in self.tok_sequence_equivalences[j].iter().enumerate() {
                let map = equiv.borrow();
                let mut feat_unified = 0usize;
                let add = |out: &mut Vec<Readings>| {
                    let Some(reading) = self.tok_sequence[j].readings.get(i) else { return };
                    if out.len() <= j || out.is_empty() {
                        out.push(Readings::new(vec![reading.clone()], 0));
                    } else {
                        out[j].add_reading(reading.clone());
                    }
                };
                if map.contains_key(UNIFY_IGNORE) {
                    add(&mut out);
                    unified_tokens_found = true;
                } else {
                    for feature in feats.keys() {
                        if map.get(feature).is_some_and(|s| s.is_empty()) {
                            feat_unified = 0;
                        } else {
                            feat_unified += 1;
                        }
                        if feat_unified == feats.len() {
                            add(&mut out);
                            unified_tokens_found = true;
                        }
                    }
                }
            }
            if !unified_tokens_found {
                return None;
            }
        }
        Some(out)
    }

    /// `isUnified(matchToken, uFeatures, lastReading)`: feed one reading of
    /// the current token; `last_reading` closes the token.
    pub fn is_unified(&mut self, token: &Reading, features: &Arc<UniFeatures>, last_reading: bool) -> bool {
        if self.in_unification {
            let sat = self.is_satisfied(token, features);
            self.uni_matched |= sat;
            self.uni_all_matched = self.uni_matched;
            if last_reading {
                self.start_next_token();
                self.uni_matched = false;
            }
            return self.uni_all_matched && self.final_unification_value(features);
        }
        self.is_satisfied(token, features);
        if last_reading {
            self.in_unification = true;
            self.uni_matched = false;
            self.start_unify();
        }
        true
    }

    /// `addNeutralElement`: a token inside `<unify-ignore>` takes part in the
    /// sequence without constraining it.
    pub fn add_neutral_element(&mut self, token: &Readings) {
        self.tok_sequence.push(token.clone());
        let map: Equiv = Rc::new(RefCell::new(HashMap::new()));
        map.borrow_mut().insert(UNIFY_IGNORE.to_string(), HashSet::new());
        self.tok_sequence_equivalences.push(token.readings.iter().map(|_| map.clone()).collect());
        self.readings_counter += 1;
    }

    /// `getFinalUnified`.
    pub fn final_unified(&self) -> Option<Vec<Readings>> {
        if self.in_unification {
            self.unified_tokens()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::pattern::PosToken;
    use super::*;

    fn pos(tag_regex: &str) -> PatternToken {
        let mut t = PatternToken::from_text("", false, false, false);
        t.pos = Some(Arc::new(PosToken::new(Some(tag_regex), true, false)));
        t
    }

    fn config() -> UnifierConfiguration {
        let mut c = UnifierConfiguration::default();
        c.set_equivalence("number", "sg", pos(".* s"));
        c.set_equivalence("number", "pl", pos(".* p"));
        c.set_equivalence("gender", "m", pos(".* m .*"));
        c.set_equivalence("gender", "f", pos(".* f .*"));
        c
    }

    fn feats(names: &[&str]) -> Arc<UniFeatures> {
        Arc::new(names.iter().map(|n| (n.to_string(), Vec::new())).collect())
    }

    fn unify(tokens: &[&[&str]], names: &[&str]) -> bool {
        let c = config();
        let mut u = c.create_unifier();
        let f = feats(names);
        let mut result = true;
        for readings in tokens {
            let mut any = false;
            for (i, tag) in readings.iter().enumerate() {
                let r = Reading::from_strs("x", Some(tag), Some("x"));
                any |= u.is_unified(&r, &f, i == readings.len() - 1);
            }
            result = any;
        }
        result
    }

    #[test]
    fn agreement_and_disagreement() {
        assert!(unify(&[&["D m s"], &["N m s"]], &["number", "gender"]));
        assert!(!unify(&[&["D m s"], &["N m p"]], &["number", "gender"]));
        assert!(!unify(&[&["D f s"], &["N m s"]], &["number", "gender"]));
        assert!(unify(&[&["D f s"], &["N m s"]], &["number"]), "only the listed features count");
    }

    #[test]
    fn one_agreeing_reading_is_enough() {
        assert!(unify(&[&["D m s"], &["N m p", "N m s"]], &["number", "gender"]));
        assert!(unify(&[&["D m s", "D f s"], &["N f s"]], &["number", "gender"]));
        assert!(unify(&[&["D m s"], &["J m s"], &["N m s"]], &["number", "gender"]));
        assert!(!unify(&[&["D m s"], &["J m s"], &["N f s"]], &["number", "gender"]));
    }
}
