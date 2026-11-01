//! Cuts the streamed LLM reply into sentences so speech synthesis can start before the reply is
//! complete.

/// Sentences shorter than this are joined with the next one ("Ja." alone sounds choppy and
/// abbreviations like "z. B." would otherwise end a sentence).
const MIN_CHARS: usize = 12;

#[derive(Default)]
pub struct SentenceSplitter {
    buf: String,
}

impl SentenceSplitter {
    /// Adds a chunk and returns every sentence that is complete now.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(end) = self.boundary() {
            let sentence = clean(&self.buf[..end]);
            self.buf.drain(..end);
            if !sentence.is_empty() {
                out.push(sentence);
            }
        }
        out
    }

    /// Returns whatever is left once the stream has ended.
    pub fn finish(&mut self) -> Option<String> {
        let rest = clean(&std::mem::take(&mut self.buf));
        (!rest.is_empty()).then_some(rest)
    }

    fn boundary(&self) -> Option<usize> {
        let mut prev: [Option<char>; 2] = [None, None];
        for (i, c) in self.buf.char_indices() {
            if c == '\n' && !self.buf[..i].trim().is_empty() {
                return Some(i + 1);
            }
            if c.is_whitespace() {
                if let Some(p) = prev[1] {
                    let ends = matches!(p, '!' | '?' | '…' | ':' | ';')
                        || (p == '.' && !is_abbreviation_or_ordinal(&self.buf[..i - 1]));
                    if ends && self.buf[..i].trim().chars().count() >= MIN_CHARS {
                        return Some(i);
                    }
                }
            }
            prev = [prev[1], Some(c)];
        }
        None
    }
}

/// "am 3. Mai", "z. B.": the word before the period is a number or a single letter.
fn is_abbreviation_or_ordinal(before_period: &str) -> bool {
    let word = before_period
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or("");
    word.chars().all(|c| c.is_ascii_digit()) || word.chars().count() == 1
}

/// Strips markdown the model sometimes emits despite the instructions.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '*' | '#' | '`' | '_'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&str]) -> Vec<String> {
        let mut s = SentenceSplitter::default();
        let mut out: Vec<String> = chunks.iter().flat_map(|c| s.push(c)).collect();
        out.extend(s.finish());
        out
    }

    #[test]
    fn splits_streamed_sentences() {
        assert_eq!(
            split(&[
                "Guten Mor",
                "gen, Veit! Heute ",
                "ist es sonnig. Viel",
                " Spass dabei."
            ]),
            [
                "Guten Morgen, Veit!",
                "Heute ist es sonnig.",
                "Viel Spass dabei."
            ]
        );
    }

    #[test]
    fn keeps_abbreviations_dates_and_short_sentences_together() {
        assert_eq!(
            split(&["Ja. Das geht z. B. am 3. Mai ganz gut. Ende"]),
            ["Ja. Das geht z. B. am 3. Mai ganz gut.", "Ende"]
        );
    }

    #[test]
    fn strips_markdown_and_splits_on_newlines() {
        assert_eq!(split(&["**Hallo**\n- erstens\n"]), ["Hallo", "- erstens"]);
    }
}
