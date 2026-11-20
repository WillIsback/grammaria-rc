// Reference outputs from ltjs (JavaScript port of LanguageTool), stage by stage,
// in the same shape as `grammaria-eval`. Run it with Bun from a directory whose
// node_modules holds @valueerror/ltjs-core and @valueerror/ltjs-lang-fr:
//
//   LTJS=/path/to/node_modules bun tools/bench/oracle_ltjs.mjs MODE eval/bench/lt.txt > out.jsonl
//
// MODE:
//   matches  [["RULE_ID", offset, length, [suggestions], "CATEGORY_ID", "FULL_ID", "issueType", "RuleClass"], ...]
//   tokens   sentences -> tokens after disambiguation:
//            ["token", [["lemma","POSTAG"], ...], flags, [chunk tags]]
//            flags: 1 immunized, 2 ignored by the speller, 4 whitespace before
//   raw      same, straight out of the tagger (before disambiguation)
// Offsets are code points. Whitespace tokens are kept (LanguageTool keeps them).
import { readFileSync } from 'node:fs';
import path from 'node:path';

const [mode, file] = process.argv.slice(2);
const root = process.env.LTJS ?? path.resolve('node_modules');
const core = (p) => import(path.join(root, '@valueerror/ltjs-core/src', p));
await core('lang/register.js'); // registers the core classes and the installed language packages
const { Languages } = await core('core/Languages.js');
const { JLanguageTool, ParagraphHandling, Level } = await core('core/JLanguageTool.js');
const { check } = await core('core/check.js');

const unescape = (line) => line.replace(/\\(.)/g, (_, c) => (c === 'n' ? '\n' : c === 't' ? '\t' : c === 'r' ? '\r' : c));
const cp = (text, i) => [...text.slice(0, i)].length;
const texts = readFileSync(file, 'utf8').split('\n');
if (texts[texts.length - 1] === '') texts.pop();

const out = [];
if (mode === 'matches') {
  // Same call as the application (default rules, default level).
  const lang = Languages.getLanguageForShortCode('fr');
  const lt = new JLanguageTool(lang, null);
  for (const line of texts) {
    const text = unescape(line);
    const matches = lt.check(text, true, ParagraphHandling.NORMAL, null, Level.DEFAULT);
    out.push(JSON.stringify(matches.map((m) => {
      const rule = m.getRule();
      const from = cp(text, m.getFromPos());
      const to = cp(text, m.getToPos());
      return [rule.getId(), from, to - from, m.getSuggestedReplacements().slice(0, 5), rule.getCategory().getId().toString(),
        rule.getFullId ? rule.getFullId() : rule.getId(), String(rule.getLocQualityIssueType()), rule.constructor.name, m.getMessage()];
    })));
  }
  void check;
} else {
  const lang = Languages.getLanguageForShortCode('fr');
  const lt = new JLanguageTool(lang, null);
  for (const line of texts) {
    const text = unescape(line);
    const sentences = lt.analyzeText(text);
    out.push(JSON.stringify(sentences.map((s) => {
      const tokens = mode === 'raw' ? s.getPreDisambigTokens() : s.getTokens();
      return tokens.map((t) => {
        const readings = t.getReadings().map((r) => [r.getLemma(), r.getPOSTag()]);
        const flags = (t.isImmunized() ? 1 : 0) | (t.isIgnoredBySpeller() ? 2 : 0) | (t.isWhitespaceBefore() ? 4 : 0);
        return [t.getToken(), readings, flags, t.getChunkTags().map(String)];
      });
    })));
  }
}
process.stdout.write(out.join('\n') + '\n');
