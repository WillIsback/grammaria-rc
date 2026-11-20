//! The French user-visible strings of the rules: the entries of LanguageTool's
//! `MessagesBundle_fr.properties` that the ported rules read with
//! `messages.getString(key)`. The constants are named after the bundle keys.

pub const DESC_COMMA_WHITESPACE: &str = "Espace devant «\u{a0},\u{a0}», «\u{a0})\u{a0}» ou après «\u{a0}(\u{a0}»";
pub const NO_SPACE_AFTER: &str = "Ne placez pas d'espace après une parenthèse ouvrante.";
pub const NO_SPACE_AROUND_QUOTES: &str = "Ne placez pas des espaces autour des guillemets.";
pub const MISSING_SPACE_AFTER_COMMA: &str = "Insérez une espace après la virgule.";
pub const NO_SPACE_BEFORE: &str = "Ne placez pas d'espace avant une parenthèse fermante.";
pub const SPACE_AFTER_COMMA: &str = "Insérez une espace après la virgule et non avant.";
pub const NO_SPACE_BEFORE_DOT: &str = "Ne placez pas d'espace avant le point.";
pub const DESC_DOUBLE_PUNCT: &str = "Deux virgules ou points consécutifs";
pub const DOUBLE_DOTS_SHORT: &str = "Deux points consécutifs";
pub const DOUBLE_COMMAS_SHORT: &str = "Deux virgules consécutives";
pub const TWO_DOTS: &str = "Deux points consécutifs";
pub const TWO_COMMAS: &str = "Deux virgules consécutives";
pub const DESC_UNPAIRED_BRACKETS: &str = "Guillemet, parenthèse ou autres symboles similaires fermants ou ouvrants manquants";
/// `{0}` stands for the missing symbol.
pub const UNPAIRED_BRACKETS: &str = "Pas de correspondance fermante ou ouvrante pour le caractère « {0} »";
pub const DESC_UPPERCASE_SENTENCE: &str = "Absence de majuscule en début de phrase";
pub const INCORRECT_CASE: &str = "Cette phrase ne commence pas par une majuscule.";
pub const CATEGORY_CASE: &str = "Majuscules";
pub const DESC_WHITESPACEREPETITION: &str = "Plusieurs espaces vides (mauvais format)";
pub const WHITESPACE_REPETITION: &str = "Faute de frappe possible : une espace est répétée";
pub const MISSING_SPACE_BETWEEN_SENTENCES: &str = "Espace manquante entre les phrases";
pub const ADD_SPACE_BETWEEN_SENTENCES: &str = "Ajoutez une espace entre les phrases.";
/// `{0}` stands for the word limit.
pub const LONG_SENTENCE_RULE_DESC: &str = "Lisibilité\u{a0}: la phrase contient plus de {0} mots";
/// `{0}` stands for the word limit.
pub const LONG_SENTENCE_RULE_MSG2: &str = "Cette phrase contient plus de {0} mots. Divisez-la afin d’obtenir une phrase plus concise, et ainsi améliorer la lisibilité de votre texte.";
/// `{0}` stands for the word limit.
pub const LONG_PARAGRAPH_RULE_DESC: &str = "Lisibilité : le paragraphe contient plus de {0} mots";
/// `{0}` stands for the word limit.
pub const LONG_PARAGRAPH_RULE_MSG: &str = "Ce paragraphe contient plus de {0} mots à la position marquée, pensez à le raccourcir.";
pub const CATEGORY_MISC: &str = "Règles de base";
pub const CATEGORY_TYPO: &str = "Faute de frappe possible";
pub const CATEGORY_PUNCTUATION: &str = "Ponctuation";
pub const CATEGORY_TYPOGRAPHY: &str = "Typographie";
pub const CATEGORY_STYLE: &str = "Style";
/// (The key is misspelt in LanguageTool.)
pub const CATEOGRY_REPETITIONS_STYLE: &str = "Répétitions (Style)";
