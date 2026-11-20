// Reference outputs of the Java Morfologik library, to compare
// src/morfologik/ with. See README.md in this directory.
//
//   Oracle dump <dict>            word<TAB>stem<TAB>tag for every entry (DictionaryIterator)
//   Oracle sequences <dict>       raw sequences of the automaton
//   Oracle lookup <dict>          stdin: words; word<TAB>stem|tag<TAB>... (DictionaryLookup)
//   Oracle speller <dict> [dist]  stdin: words; isMisspelled, isInDictionary, getFrequency,
//                                 findReplacementCandidates, replaceRunOnWords, findSimilarWords,
//                                 with a new Speller before each search, as LanguageTool does
//   Oracle speller-shared <dict> [dist]   same with one Speller for the whole list
//   Oracle nfd | otheralpha | letternumbers | chars   Unicode data of the JDK
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.text.Normalizer;
import java.util.List; import java.util.ArrayList;
import morfologik.stemming.*;
import morfologik.speller.Speller;

public class Oracle {
  static String s(CharSequence c) { return c == null ? "" : c.toString(); }

  public static void main(String[] a) throws Exception {
    PrintStream out = new PrintStream(new BufferedOutputStream(new FileOutputStream(FileDescriptor.out), 1 << 16), false, "UTF-8");
    String mode = a[0];
    if (mode.equals("nfd")) {
      for (int c = 0; c < 0x10000; c++) {
        if (c >= 0xD800 && c <= 0xDFFF) continue;
        if (c >= 0xAC00 && c <= 0xD7A3) continue;
        String n = Normalizer.normalize(Character.toString((char) c), Normalizer.Form.NFD);
        if (n.charAt(0) != c) out.println(Integer.toHexString(c) + " " + Integer.toHexString(n.charAt(0)));
      }
      out.flush();
      return;
    }
    if (mode.equals("otheralpha") || mode.equals("letternumbers")) {
      // BMP ranges that have the Alphabetic property without being letters:
      // letter numbers (Nl), or everything else (marks, circled letters).
      boolean wantNl = mode.equals("letternumbers");
      int start = -1;
      for (int c = 0; c <= 0x10000; c++) {
        boolean in = c < 0x10000 && Character.isAlphabetic(c) && !Character.isLetter((char) c)
            && ((Character.getType((char) c) == Character.LETTER_NUMBER) == wantNl);
        if (in && start < 0) start = c;
        if (!in && start >= 0) {
          out.println(Integer.toHexString(start) + " " + Integer.toHexString(c - 1));
          start = -1;
        }
      }
      out.flush();
      return;
    }
    if (mode.equals("chars")) {
      for (int c = 0; c < 0x10000; c++) {
        char ch = (char) c;
        int t = Character.getType(ch);
        boolean alpha = ((1 << Character.UPPERCASE_LETTER | 1 << Character.LOWERCASE_LETTER | 1 << Character.TITLECASE_LETTER
          | 1 << Character.MODIFIER_LETTER | 1 << Character.OTHER_LETTER | 1 << Character.LETTER_NUMBER) >> t & 1) != 0;
        out.println(Integer.toHexString(c) + " " + (Character.isLetter(ch) ? 1 : 0) + (Character.isLowerCase(ch) ? 1 : 0)
          + (Character.isUpperCase(ch) ? 1 : 0) + (Character.isDigit(ch) ? 1 : 0) + (alpha ? 1 : 0)
          + " " + Integer.toHexString(Character.toUpperCase(ch)) + " " + Integer.toHexString(Character.toLowerCase(ch)));
      }
      out.flush();
      return;
    }
    if (mode.equals("encode")) {
      // stdin: word<TAB>stem; output, for each stem encoder:
      // ENCODER<TAB>word<TAB>stem<TAB>hex of the encoded stem.
      BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
      String line;
      while ((line = in.readLine()) != null) {
        String[] p = line.split("\t", -1);
        byte[] src = p[0].getBytes(StandardCharsets.UTF_8);
        byte[] dst = p[1].getBytes(StandardCharsets.UTF_8);
        for (EncoderType t : EncoderType.values()) {
          java.nio.ByteBuffer enc = t.get().encode(java.nio.ByteBuffer.allocate(0), java.nio.ByteBuffer.wrap(src), java.nio.ByteBuffer.wrap(dst));
          StringBuilder hex = new StringBuilder();
          for (int i = enc.position(); i < enc.limit(); i++) hex.append(String.format("%02x", enc.get(i) & 0xff));
          out.println(t.name() + "\t" + p[0] + "\t" + p[1] + "\t" + hex);
        }
      }
      out.flush();
      return;
    }
    Dictionary dict = Dictionary.read(Paths.get(a[1]));
    if (mode.equals("dump")) {
      DictionaryLookup dl = new DictionaryLookup(dict);
      for (WordData wd : dl) {
        out.println(s(wd.getWord()) + "\t" + s(wd.getStem()) + "\t" + s(wd.getTag()));
      }
      out.flush();
      return;
    }
    if (mode.equals("sequences")) {
      // Raw sequences of the automaton, one per line (for dictionaries
      // DictionaryIterator cannot read, e.g. word lists without separator).
      for (java.nio.ByteBuffer bb : dict.fsa) {
        out.println(new String(bb.array(), bb.position(), bb.remaining(), StandardCharsets.UTF_8));
      }
      out.flush();
      return;
    }
    BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
    String w;
    if (mode.equals("lookup")) {
      DictionaryLookup dl = new DictionaryLookup(dict);
      while ((w = in.readLine()) != null) {
        StringBuilder sb = new StringBuilder(w);
        for (WordData wd : dl.lookup(w)) {
          sb.append('\t').append(s(wd.getStem())).append('|').append(s(wd.getTag()));
        }
        out.println(sb);
      }
    } else if (mode.equals("speller") || mode.equals("speller-shared")) {
      int dist = a.length > 2 ? Integer.parseInt(a[2]) : 1;
      boolean shared = mode.equals("speller-shared");
      Speller sp = new Speller(dict, dist);
      while ((w = in.readLine()) != null) {
        if (!shared) sp = new Speller(dict, dist);
        StringBuilder sb = new StringBuilder(w);
        sb.append("\tM=").append(sp.isMisspelled(w) ? 1 : 0);
        sb.append("\tD=").append(sp.isInDictionary(w) ? 1 : 0);
        sb.append("\tF=").append(sp.getFrequency(w));
        if (!shared) sp = new Speller(dict, dist);
        sb.append("\tR=");
        boolean first = true;
        for (Speller.CandidateData cd : sp.findReplacementCandidates(w)) {
          if (!first) sb.append('|');
          first = false;
          sb.append(cd.getWord()).append('/').append(cd.getDistance());
        }
        sb.append("\tO=").append(String.join("|", sp.replaceRunOnWords(w)));
        if (!shared) sp = new Speller(dict, dist);
        sb.append("\tS=").append(String.join("|", sp.findSimilarWords(w)));
        out.println(sb);
      }
    }
    out.flush();
  }
}
