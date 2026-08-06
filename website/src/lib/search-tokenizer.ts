/**
 * The tokenizer the search index is built with — and, necessarily, the one it
 * is queried with. A stemmed, stopword-stripped index is markedly smaller than
 * a literal one, but only matches a query tokenised the same way, so this is
 * the single definition both `build/search-index.ts` and `lib/search.ts` use.
 */
export async function tokenizerComponents() {
  const [{ stemmer }, { stopwords }] = await Promise.all([
    import("@zbsearch/stemmers/english"),
    import("@zbsearch/stopwords/english"),
  ]);

  return { tokenizer: { language: "english", stemming: true, stemmer, stopWords: stopwords } };
}
