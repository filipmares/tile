// Pure text matching for the settings search. No DOM here, so the rules for
// what counts as a match can be read (and reasoned about) on their own.

/**
 * Folds text for comparison: lower case, accents stripped, runs of
 * whitespace collapsed. "Behaviour", "BEHAVIOUR" and "behavíour" all fold to
 * the same string.
 */
export function foldText(text: string): string {
  return text
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .trim();
}

/** Splits a query into folded terms. An empty array means "no query". */
export function queryTerms(query: string): string[] {
  const folded = foldText(query);
  return folded === "" ? [] : folded.split(" ");
}

/**
 * Whether every term appears somewhere in `haystack`, in any order. Words
 * can be typed out of order ("gap top" finds "Top screen-edge gap"), and a
 * partial word still matches while the user is typing it.
 */
export function matchesTerms(haystack: string, terms: readonly string[]): boolean {
  if (terms.length === 0) return true;
  const folded = foldText(haystack);
  return terms.every((term) => folded.includes(term));
}
